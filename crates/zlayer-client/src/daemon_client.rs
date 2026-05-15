//! HTTP client for CLI-to-daemon communication.
//!
//! Provides a typed [`DaemonClient`] that communicates with the `zlayer serve`
//! daemon. On Unix platforms the transport is HTTP-over-Unix-domain-socket
//! (platform-dependent path; see [`default_socket_path()`]). On Windows it is
//! HTTP-over-TCP-loopback at `127.0.0.1:3669`.
//!
//! If the daemon is not running, [`DaemonClient::connect`] will auto-start it
//! and wait with exponential backoff until the socket / TCP port becomes
//! available.

#[cfg(unix)]
use std::future::Future;
#[cfg(windows)]
use std::net::SocketAddr;
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
#[cfg(unix)]
use std::pin::Pin;
#[cfg(unix)]
use std::task::{Context, Poll};
use std::time::Duration;

use anyhow::{bail, Context as _, Result};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::Uri;
#[cfg(windows)]
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
#[cfg(unix)]
use tokio::net::UnixStream;
use tracing::{debug, info};
use zlayer_types::api::auth::{BootstrapRequest, LoginRequest, LoginResponse, UserView};
use zlayer_types::api::containers::{
    ContainerExecResponse, ContainerUpdateRequest, ContainerUpdateResponse, CreateContainerRequest,
};
use zlayer_types::api::images::{
    CommitContainerRequest, CommitContainerResponse, ImageHistoryEntryDto, ImageInfoDto,
    ImageSearchResultDto, ImportImageResponse, PruneResultDto, PullImageResponse,
};
use zlayer_types::api::secrets::{
    RevealAllSecretsResponse, RotateSecretResponse, SecretMetadataResponse,
};
use zlayer_types::api::users::{CreateUserRequest, SetPasswordRequest, UpdateUserRequest};
use zlayer_types::client::{BuildHandle, BuildSpec};
use zlayer_types::storage::{StoredEnvironment, StoredVariable};

/// Default daemon endpoint.
///
/// On macOS: `~/.zlayer/run/zlayer.sock` (Unix-domain socket path).
/// On Linux: `/var/run/zlayer.sock` (Unix-domain socket path).
/// On Windows: `tcp://127.0.0.1:3669` (TCP loopback URL).
#[must_use]
pub fn default_socket_path() -> String {
    zlayer_paths::ZLayerDirs::default_socket_path()
}

// ---------------------------------------------------------------------------
// Unix socket connector (tower::Service<Uri>)
// ---------------------------------------------------------------------------

/// A [`tower::Service`] connector that routes every request to a fixed Unix
/// domain socket, regardless of the URI's host/port.  This lets us reuse the
/// standard `hyper_util::client::legacy::Client` for HTTP-over-UDS.
#[cfg(unix)]
#[derive(Clone)]
struct UnixConnector {
    socket_path: PathBuf,
}

#[cfg(unix)]
impl UnixConnector {
    fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }
}

#[cfg(unix)]
impl tower::Service<Uri> for UnixConnector {
    type Response = hyper_util::rt::TokioIo<UnixStream>;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _uri: Uri) -> Self::Future {
        let path = self.socket_path.clone();
        Box::pin(async move {
            let stream = UnixStream::connect(&path).await?;
            Ok(hyper_util::rt::TokioIo::new(stream))
        })
    }
}

// ---------------------------------------------------------------------------
// Platform-specific connector type alias
// ---------------------------------------------------------------------------

/// Transport connector used by [`DaemonClient`]. On Unix this is the custom
/// [`UnixConnector`] (HTTP-over-UDS). On Windows it is `hyper_util`'s standard
/// [`HttpConnector`] (HTTP-over-TCP loopback).
#[cfg(unix)]
type DaemonConnector = UnixConnector;
#[cfg(windows)]
type DaemonConnector = HttpConnector;

// ---------------------------------------------------------------------------
// Windows endpoint helpers
// ---------------------------------------------------------------------------

/// Parse a daemon endpoint string into a [`SocketAddr`].
///
/// Accepts either `tcp://127.0.0.1:3669` (the shape returned by
/// [`default_socket_path()`] on Windows) or a bare `127.0.0.1:3669`.
#[cfg(windows)]
fn parse_tcp_url(raw: &str) -> Result<SocketAddr> {
    let trimmed = raw.strip_prefix("tcp://").unwrap_or(raw);
    trimmed
        .parse::<SocketAddr>()
        .map_err(|e| anyhow::anyhow!("invalid daemon endpoint {raw:?}: {e}"))
}

/// Read the locally-persisted admin bearer token, if any.
///
/// Windows has no Unix-socket peer-credential check, so the daemon
/// authenticates local admin clients via a file-backed bearer token.
///
/// The server persists the raw JWT to
/// [`zlayer_paths::default_admin_bearer_path()`] on startup (see
/// `zlayer-api::server::persist_admin_bearer`). This reads that file and
/// returns the trimmed contents, or `None` if the file is missing, empty, or
/// unreadable (e.g. permission denied). Callers then fall back to whatever
/// the daemon's default auth path permits.
#[cfg(windows)]
fn read_local_bearer() -> Option<String> {
    let path = zlayer_paths::default_admin_bearer_path();
    let raw = std::fs::read_to_string(&path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

// ---------------------------------------------------------------------------
// Public response types
// ---------------------------------------------------------------------------

/// Response body returned by the daemon's `POST /api/v1/containers` endpoint.
///
/// The daemon currently replies with a [`zlayer_types::api::containers::ContainerInfo`]
/// — this struct accepts the subset of those fields the Docker-compat shim
/// needs (`id`, `name`) plus an additive `warnings` array that the daemon may
/// surface in the future. Both `warnings` and `name` are tolerant defaults so
/// older daemon builds (which never emit them) continue to deserialize.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CreateContainerResponse {
    /// Identifier of the newly-created container, as assigned by the daemon.
    pub id: String,
    /// Non-fatal warnings raised during container creation. Always present on
    /// the wire as an array; defaults to empty when the daemon omits the field.
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Human-readable name, when the request supplied one.
    #[serde(default)]
    pub name: Option<String>,
}

/// Response body returned by the daemon's
/// `POST /api/v1/containers/{id}/wait` Docker-compat endpoint.
///
/// The shape mirrors Docker's `/containers/{id}/wait`: a numeric exit
/// `status_code` plus an optional nested `error` carrying a textual message
/// when the wait itself failed (e.g. the container was removed before
/// reaching the requested condition).
///
/// Wired in this crate so SDK consumers can deserialize the response without
/// pulling in `zlayer-api`'s richer
/// [`zlayer_types::api::containers::ContainerWaitResponse`] (which describes
/// the native `GET /wait` endpoint and carries additional classification
/// fields).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct WaitContainerResponse {
    /// Container exit code, or `0` when the container exited cleanly. When
    /// the container was killed by signal `N`, this is typically `128 + N`,
    /// matching Docker's convention.
    pub status_code: i64,
    /// Optional error envelope surfaced when the wait itself failed. Absent
    /// on a normal exit; present when the daemon could not honour the
    /// requested wait condition (for example, the container was removed
    /// before it reached `not-running`).
    #[serde(default)]
    pub error: Option<WaitContainerError>,
}

/// Error envelope nested inside [`WaitContainerResponse::error`].
#[derive(Debug, Clone, serde::Deserialize)]
pub struct WaitContainerError {
    /// Human-readable description of why the wait failed.
    pub message: String,
}

// ---------------------------------------------------------------------------
// Streaming wire DTOs (logs / stats / image pull)
// ---------------------------------------------------------------------------
//
// These mirror the equivalent runtime types defined in
// `zlayer_agent::runtime` (`LogChunk`, `LogChannel`, `LogsStreamOptions`,
// `StatsSample`, `PullProgress`) but live here so SDK consumers can use the
// streaming `DaemonClient::stream_*` methods without taking a direct
// dependency on the heavy `zlayer-agent` crate (which drags in libcontainer,
// youki, oci-client, etc.).
//
// Wire shape is intentionally JSON-friendly: every field is serde-derived
// and the `PullProgress` enum uses an internal `type` tag so a decoder can
// distinguish `Status` from `Done` without out-of-band context. The shapes
// match what the daemon's NDJSON streaming endpoints
// (`GET /api/v1/containers/{id}/stats?stream=true`,
//  `POST /api/v1/images/pull?stream=true`) emit.

/// Which standard stream a [`LogChunk`] originated from.
///
/// Mirrors `zlayer_agent::runtime::LogChannel`. Re-exported at the crate
/// root so SDK callers can deserialize log NDJSON lines without depending
/// on `zlayer-agent`.
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

/// One chunk of container log output, as it appears on the wire when
/// `format=json` is requested from the daemon's logs endpoint.
///
/// Mirrors `zlayer_agent::runtime::LogChunk`. `bytes` is encoded as a UTF-8
/// `String` rather than raw `bytes::Bytes` because JSON has no native
/// binary type and most container output is human-readable text.
/// Consumers that need binary-safe access should call
/// [`DaemonClient::stream_container_logs`] with `format_raw = true` and
/// parse the underlying byte stream themselves.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LogChunk {
    /// Which standard stream produced this chunk.
    pub channel: LogChannel,
    /// Decoded UTF-8 payload. Backends that emit non-UTF-8 bytes should
    /// produce them lossy or escape them.
    pub bytes: String,
    /// When the runtime reported this chunk, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
}

/// Options accepted by [`DaemonClient::stream_container_logs`].
///
/// Mirrors `zlayer_agent::runtime::LogsStreamOptions`. Re-exported from the
/// client so callers can construct request parameters with the same shape
/// the daemon trait method accepts on the server side.
#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_excessive_bools)] // mirrors Docker's logs query params 1:1
pub struct LogsStreamOptions {
    /// Continue streaming after the current end-of-log marker.
    pub follow: bool,
    /// Tail the last N lines before starting to stream.
    pub tail: Option<u64>,
    /// Earliest timestamp (Unix seconds) to include.
    pub since: Option<i64>,
    /// Latest timestamp (Unix seconds) to include.
    pub until: Option<i64>,
    /// Include per-chunk timestamps in the response.
    pub timestamps: bool,
    /// Include stdout in the stream.
    pub stdout: bool,
    /// Include stderr in the stream.
    pub stderr: bool,
}

/// One periodic resource-usage sample returned by
/// [`DaemonClient::stream_container_stats`].
///
/// Mirrors `zlayer_agent::runtime::StatsSample`. Field names match the
/// `snake_case` JSON shape emitted by the daemon's streaming stats endpoint.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StatsSample {
    /// Cumulative container CPU time consumed, in nanoseconds.
    pub cpu_total_ns: u64,
    /// Cumulative system CPU time observed at the same moment, in
    /// nanoseconds.
    pub cpu_system_ns: u64,
    /// Number of CPUs currently online for this container.
    pub online_cpus: u32,
    /// Resident memory currently in use, in bytes.
    pub mem_used_bytes: u64,
    /// Memory limit configured on the container, in bytes. `0` when no
    /// limit is set.
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
    /// Configured pids limit, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pids_limit: Option<u64>,
    /// Wallclock time the sample was taken.
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// One progress event emitted by [`DaemonClient::stream_image_pull`].
///
/// Mirrors `zlayer_agent::runtime::PullProgress`. Uses serde's internally-
/// tagged enum representation (`{"type":"status",...}` vs
/// `{"type":"done",...}`) so a single NDJSON parser can distinguish the
/// two variants without out-of-band context.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PullProgress {
    /// Progress update for an in-flight layer or stage.
    Status {
        /// Layer ID or other backend-specific identifier, when available.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// Human-readable status text.
        status: String,
        /// Pre-formatted progress bar, when emitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        progress: Option<String>,
        /// Bytes transferred so far for this layer, when reported.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        current: Option<u64>,
        /// Expected total bytes for this layer, when reported.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total: Option<u64>,
    },
    /// Pull completed successfully.
    Done {
        /// Resolved canonical image reference.
        reference: String,
        /// Content-addressed digest, when the backend reports one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        digest: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Exec wire DTOs
// ---------------------------------------------------------------------------
//
// These mirror the agent-side types defined in `zlayer_agent::runtime`
// (`ExecOptions`, `ExecHandle`) and the daemon-side `ExecInstance`
// registry struct in `zlayer_api::handlers::exec_instances`. They live in
// `zlayer-client` so SDK consumers can drive the Docker-style exec flow
// (create -> start -> resize -> inspect) without taking a direct
// dependency on the heavy `zlayer-agent` crate.
//
// The shapes match the daemon's request/response bodies for the new
// `POST /api/v1/containers/{id}/exec`, `GET /api/v1/exec/{id}/json`,
// `POST /api/v1/exec/{id}/resize`, `POST /api/v1/exec/{id}/start`, and
// `POST /api/v1/containers/{id}/resize` endpoints.

/// Configuration for a new exec instance, sent as the JSON body of
/// `POST /api/v1/containers/{id}/exec`.
///
/// Mirrors `zlayer_agent::runtime::ExecOptions` field-for-field. The agent
/// struct does not derive `serde`, so this client-side copy is the canonical
/// wire shape: keep the two in sync when fields are added.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[allow(clippy::struct_excessive_bools)] // mirrors Docker's `ExecConfig` 1:1
pub struct ExecOptions {
    /// The argv vector for the exec'd process (`command[0]` is the binary).
    pub command: Vec<String>,
    /// Extra environment variables, in `KEY=VALUE` form. Merged into the
    /// container's existing env on the runtime side.
    #[serde(default)]
    pub env: Vec<String>,
    /// Optional working directory inside the container. `None` keeps the
    /// container's default `WORKDIR`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    /// Optional `user[:group]` override. `None` keeps the container's
    /// configured user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Run the exec with privileged capabilities (Docker `Privileged`).
    #[serde(default)]
    pub privileged: bool,
    /// Allocate a TTY for the exec'd process (Docker `Tty`).
    #[serde(default)]
    pub tty: bool,
    /// Attach stdin so the caller can write to the process (Docker `AttachStdin`).
    #[serde(default)]
    pub attach_stdin: bool,
    /// Attach stdout so the caller receives the process's stdout on the
    /// readable half of the duplex stream (Docker `AttachStdout`).
    #[serde(default)]
    pub attach_stdout: bool,
    /// Attach stderr so the caller receives the process's stderr on the
    /// readable half of the duplex stream (Docker `AttachStderr`).
    #[serde(default)]
    pub attach_stderr: bool,
}

/// Native [`zlayer_agent::runtime::ContainerId`] flavoured for the wire.
///
/// We re-spell the struct here so the client doesn't pull in `zlayer-agent`
/// just to deserialize an exec inspect body. Field names match the
/// `serde::Serialize` / `serde::Deserialize` derive on the agent's
/// `ContainerId`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExecContainerRef {
    /// Logical service name (deployment-scoped) the exec is bound to.
    pub service: String,
    /// Replica index within the service.
    pub replica: u32,
}

/// Wire shape returned by `GET /api/v1/exec/{id}/json`, matching the
/// `ExecInstance` struct in `zlayer_api::handlers::exec_instances`.
///
/// All timestamp fields are RFC 3339 UTC strings; pre-start fields
/// (`started_at`, `finished_at`, `exit_code`) are `None` until the exec
/// has progressed through the corresponding lifecycle step.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExecInstanceJson {
    /// 64-char lowercase hex ID. Matches Docker's exec-ID shape.
    pub id: String,
    /// Container the exec was created against.
    pub container_id: ExecContainerRef,
    /// The planned exec configuration captured at create time.
    pub options: ExecOptions,
    /// When the instance was created (always set).
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// When the runtime accepted the exec start. `None` until the exec has
    /// been started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// When the exec'd process exited. `None` until the exec has finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Exit code reported by the runtime. `None` until the exec has
    /// finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

/// Response body of `POST /api/v1/containers/{id}/exec`.
///
/// The daemon uses Docker's response convention (a single `Id` field) but
/// also accepts lowercase `id` for forward compatibility with non-Docker
/// callers. Both keys deserialize to the same field via `serde(alias)`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CreateExecResponse {
    /// 64-char lowercase hex exec ID.
    #[serde(alias = "Id")]
    pub id: String,
}

/// Bidirectional connection returned by [`DaemonClient::start_exec_pty`].
///
/// Bundles the four pieces a long-lived interactive exec session needs:
///
/// 1. The readable half of the WebSocket as an async stream of binary
///    payloads (`stdout` / `stderr`). The caller pumps this on one task to
///    forward output to the user's terminal.
/// 2. The writable half of the WebSocket, exposed via the [`ExecPtyWriter`]
///    handle. Calling [`ExecPtyWriter::send_stdin`] queues a binary frame;
///    [`ExecPtyWriter::close`] sends a normal-closure frame. The caller
///    typically pumps stdin on a second task.
/// 3. A bounded `mpsc::Sender<(rows, cols)>` so a SIGWINCH handler can
///    forward terminal-resize hints. The session loop translates each
///    `(rows, cols)` pair into a `{"resize":{"rows":..,"cols":..}}` JSON
///    text frame.
/// 4. A boxed exit future that resolves once the daemon sends a normal
///    close frame, with the exit code parsed from the close frame's
///    `reason` field.
///
/// `ExecPtyConnection` deliberately does not implement `Debug` / `Clone`
/// because every field holds either a trait object or an owned channel.
pub struct ExecPtyConnection {
    /// Stream of binary frames read from the daemon (stdout / stderr,
    /// already framed by the daemon when `tty=false`). Each `Bytes` item is
    /// one binary WebSocket frame; non-binary frames (text/ping/pong) are
    /// filtered upstream and never surface here.
    pub reader: std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<Bytes>> + Send>>,
    /// Writable handle for queuing stdin payloads onto the WebSocket. Calls
    /// to [`ExecPtyWriter::send_stdin`] are forwarded to the WebSocket as
    /// binary frames; the underlying channel is bounded so a stuck daemon
    /// can't make the caller buffer unbounded stdin.
    pub writer: ExecPtyWriter,
    /// Sender for `(rows, cols)` PTY-resize events. Closed once the
    /// session ends.
    pub resize: tokio::sync::mpsc::Sender<(u16, u16)>,
    /// Future that resolves with the exit code parsed from the daemon's
    /// close frame. Resolves with `None` when the daemon closes without
    /// surfacing an exit code (e.g. transport-level termination).
    pub exit: std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<i32>>> + Send>>,
}

/// Owned writable side of an [`ExecPtyConnection`].
///
/// Wraps the bounded `mpsc::Sender` that the session loop drains to produce
/// outbound WebSocket binary frames. Constructed exclusively by
/// [`DaemonClient::start_exec_pty`]; clones share the same channel.
#[derive(Clone)]
pub struct ExecPtyWriter {
    tx: tokio::sync::mpsc::Sender<ExecPtyOutbound>,
}

impl ExecPtyWriter {
    /// Queue `data` as a binary WebSocket frame to the daemon's stdin.
    ///
    /// # Errors
    ///
    /// Returns `Err` once the underlying session loop has shut down (for
    /// example because the daemon closed the connection, or the caller
    /// already called [`Self::close`]).
    pub async fn send_stdin(&self, data: Bytes) -> Result<()> {
        self.tx
            .send(ExecPtyOutbound::Stdin(data))
            .await
            .map_err(|_| anyhow::anyhow!("exec PTY session closed"))
    }

    /// Send a normal-closure (1000) close frame to the daemon and shut the
    /// session down. Idempotent: subsequent calls return an error because
    /// the channel will have been dropped on the session-loop side.
    ///
    /// # Errors
    ///
    /// Returns `Err` when the session loop has already shut down (e.g. a
    /// previous `close` call ran, or the daemon closed the connection).
    pub async fn close(&self) -> Result<()> {
        self.tx
            .send(ExecPtyOutbound::Close)
            .await
            .map_err(|_| anyhow::anyhow!("exec PTY session already closed"))
    }
}

/// Internal commands sent from the public [`ExecPtyWriter`] /
/// [`ExecPtyConnection::resize`] handles to the session loop that owns the
/// underlying `WebSocketStream`.
#[derive(Debug)]
enum ExecPtyOutbound {
    /// Forward `data` as a binary WebSocket frame.
    Stdin(Bytes),
    /// Forward a `{"resize":{"rows":r,"cols":c}}` text frame.
    Resize { rows: u16, cols: u16 },
    /// Send a 1000 close frame and end the session.
    Close,
}

// ---------------------------------------------------------------------------
// DaemonClient
// ---------------------------------------------------------------------------

/// HTTP client that communicates with the zlayer daemon.
///
/// All API methods send HTTP requests to the daemon's REST API. On Unix the
/// URI host component is set to `localhost` (ignored by the connector, which
/// always dials the configured Unix-domain socket). On Windows the host
/// component is the TCP endpoint (`127.0.0.1:3669`).
pub struct DaemonClient {
    /// Inner hyper client wired to the platform-specific connector.
    client: Client<DaemonConnector, Full<Bytes>>,
    /// Path to the Unix socket file (Unix only).
    #[cfg(unix)]
    socket_path: PathBuf,
    /// TCP loopback endpoint (Windows only).
    #[cfg(windows)]
    endpoint: SocketAddr,
    /// Bearer token read from the local admin-token file (Windows only).
    ///
    /// Windows has no Unix-socket peer-credential check, so the daemon
    /// authenticates local admin clients via a file-backed bearer token.
    /// Batch B of F-7b will persist this token; for now this is always `None`.
    #[cfg(windows)]
    bearer: Option<String>,
}

/// Distinguishes the ways a probe of the daemon's local socket / endpoint
/// can fail.  This lets callers tell "daemon is not running" from "daemon is
/// running but the current user has no permission to talk to it" — the
/// latter is common on freshly-installed Linux systems where the user has
/// been added to the `zlayer` group but the current shell session has not
/// picked up the new group membership yet (`newgrp zlayer` or re-login).
pub enum DaemonReachability {
    /// The daemon answered a health probe successfully.
    ///
    /// Boxed because [`DaemonClient`] is ~296 bytes and dwarfs the other
    /// variants; otherwise clippy fires `large_enum_variant`.
    Reachable(Box<DaemonClient>),
    /// The socket file (or endpoint) does not exist — daemon is not running.
    SocketMissing,
    /// The socket file exists but `connect()` returned `EACCES`.  Daemon is
    /// running, but the calling process lacks group permission to reach it.
    PermissionDenied,
    /// The socket file exists but `connect()` returned `ECONNREFUSED`.
    /// Often indicates a stale socket file from a crashed daemon.
    ConnectionRefused,
    /// Any other I/O or transport error.
    Other(std::io::Error),
}

impl std::fmt::Debug for DaemonReachability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Reachable(_) => f.write_str("Reachable(DaemonClient)"),
            Self::SocketMissing => f.write_str("SocketMissing"),
            Self::PermissionDenied => f.write_str("PermissionDenied"),
            Self::ConnectionRefused => f.write_str("ConnectionRefused"),
            Self::Other(e) => f.debug_tuple("Other").field(e).finish(),
        }
    }
}

impl DaemonReachability {
    /// Convenience: collapse to `Option<DaemonClient>` for callers that
    /// don't care about the reason.
    #[must_use]
    pub fn into_option(self) -> Option<DaemonClient> {
        match self {
            Self::Reachable(c) => Some(*c),
            _ => None,
        }
    }

    /// Convenience: did we find a daemon to talk to?
    #[must_use]
    pub fn is_reachable(&self) -> bool {
        matches!(self, Self::Reachable(_))
    }
}

#[allow(clippy::missing_errors_doc)]
impl DaemonClient {
    // ------------------------------------------------------------------
    // Construction / connection
    // ------------------------------------------------------------------

    /// Connect to the daemon, auto-starting it if it is not already running.
    ///
    /// On Unix: checks for the Unix socket at [`default_socket_path()`]. If the
    /// socket does not exist and no daemon PID is found, `zlayer serve --daemon`
    /// is spawned and the function polls the socket with exponential backoff
    /// (100 ms -> 200 ms -> 400 ms -> ... -> 1 s, max 20 attempts, ~10 s total).
    ///
    /// On Windows: connects to `tcp://127.0.0.1:3669` (or whatever
    /// [`default_socket_path()`] returns). Auto-start uses the same spawn
    /// logic, but polls the TCP port rather than a socket file.
    #[cfg(unix)]
    pub async fn connect() -> Result<Self> {
        Self::connect_to(&default_socket_path()).await
    }

    /// Connect to the daemon, auto-starting it if it is not already running.
    ///
    /// See Unix variant above for semantics.
    #[cfg(windows)]
    pub async fn connect() -> Result<Self> {
        let raw = default_socket_path();
        Self::connect_to(raw).await
    }

    /// Try to connect to a running daemon without auto-starting.
    ///
    /// Returns `Ok(Some(client))` if the daemon is running and healthy,
    /// `Ok(None)` if the daemon is not running, or `Err` on unexpected errors.
    #[cfg(unix)]
    pub async fn try_connect() -> Result<Option<Self>> {
        Self::try_connect_to(&default_socket_path()).await
    }

    /// Try to connect to a running daemon without auto-starting.
    #[cfg(windows)]
    pub async fn try_connect() -> Result<Option<Self>> {
        let raw = default_socket_path();
        match parse_tcp_url(&raw) {
            Ok(endpoint) => {
                let bearer = read_local_bearer();
                match Self::try_build_windows(endpoint, bearer).await {
                    Ok(client) => Ok(Some(client)),
                    Err(_) => Ok(None),
                }
            }
            Err(_) => Ok(None),
        }
    }

    /// Like [`try_connect`](Self::try_connect) but with a custom socket path.
    #[cfg(unix)]
    pub async fn try_connect_to(socket_path: impl AsRef<Path>) -> Result<Option<Self>> {
        let socket_path = socket_path.as_ref().to_path_buf();

        if socket_path.exists() {
            match Self::try_build(&socket_path).await {
                Ok(client) => Ok(Some(client)),
                Err(_) => Ok(None),
            }
        } else {
            Ok(None)
        }
    }

    /// Probe the daemon at `socket_path` and return a categorized result.
    ///
    /// Unlike [`try_connect_to`](Self::try_connect_to), this does NOT collapse
    /// permission and connection-refused errors into "not running" — callers
    /// (status display, install readiness probe) can branch on the reason.
    #[cfg(unix)]
    pub async fn probe(socket_path: impl AsRef<Path>) -> DaemonReachability {
        use std::io::ErrorKind;

        let socket_path = socket_path.as_ref().to_path_buf();

        // Quick stat: if the path doesn't exist, the daemon is not running.
        // Note: `Path::exists()` returns false for both ENOENT and EACCES on
        // the parent directory, so we use `metadata()` to disambiguate when
        // possible.
        match std::fs::metadata(&socket_path) {
            Err(e) if e.kind() == ErrorKind::NotFound => {
                return DaemonReachability::SocketMissing;
            }
            Err(e) if e.kind() == ErrorKind::PermissionDenied => {
                return DaemonReachability::PermissionDenied;
            }
            Err(e) => {
                return DaemonReachability::Other(e);
            }
            Ok(_) => {}
        }

        // Attempt the connect + health check.
        match Self::try_build(&socket_path).await {
            Ok(client) => DaemonReachability::Reachable(Box::new(client)),
            Err(e) => {
                // Walk the error chain looking for an io::Error so we can
                // bucket on ErrorKind.
                let mut err_kind = None;
                for cause in e.chain() {
                    if let Some(io_err) = cause.downcast_ref::<std::io::Error>() {
                        err_kind = Some(io_err.kind());
                        break;
                    }
                }
                match err_kind {
                    Some(ErrorKind::PermissionDenied) => DaemonReachability::PermissionDenied,
                    Some(ErrorKind::ConnectionRefused) => DaemonReachability::ConnectionRefused,
                    Some(ErrorKind::NotFound) => DaemonReachability::SocketMissing,
                    _ => DaemonReachability::Other(std::io::Error::other(format!("{e}"))),
                }
            }
        }
    }

    /// Like [`try_connect`](Self::try_connect) but with a custom TCP endpoint.
    ///
    /// Accepts either `tcp://127.0.0.1:3669` or bare `127.0.0.1:3669`.
    #[cfg(windows)]
    pub async fn try_connect_to(endpoint: impl Into<String>) -> Result<Option<Self>> {
        let raw = endpoint.into();
        match parse_tcp_url(&raw) {
            Ok(parsed) => {
                let bearer = read_local_bearer();
                match Self::try_build_windows(parsed, bearer).await {
                    Ok(client) => Ok(Some(client)),
                    Err(_) => Ok(None),
                }
            }
            Err(_) => Ok(None),
        }
    }

    /// Probe the daemon TCP endpoint and return a categorized result.
    #[cfg(windows)]
    pub async fn probe(endpoint: impl Into<String>) -> DaemonReachability {
        use std::io::ErrorKind;

        let raw = endpoint.into();
        let parsed = match parse_tcp_url(&raw) {
            Ok(p) => p,
            Err(e) => return DaemonReachability::Other(std::io::Error::other(format!("{e}"))),
        };
        let bearer = read_local_bearer();
        match Self::try_build_windows(parsed, bearer).await {
            Ok(client) => DaemonReachability::Reachable(Box::new(client)),
            Err(e) => {
                let mut err_kind = None;
                for cause in e.chain() {
                    if let Some(io_err) = cause.downcast_ref::<std::io::Error>() {
                        err_kind = Some(io_err.kind());
                        break;
                    }
                }
                match err_kind {
                    Some(ErrorKind::PermissionDenied) => DaemonReachability::PermissionDenied,
                    Some(ErrorKind::ConnectionRefused) => DaemonReachability::ConnectionRefused,
                    Some(ErrorKind::NotFound) => DaemonReachability::SocketMissing,
                    _ => DaemonReachability::Other(std::io::Error::other(format!("{e}"))),
                }
            }
        }
    }

    /// Like [`connect`](Self::connect) but with a custom socket path.
    #[cfg(unix)]
    pub async fn connect_to(socket_path: impl AsRef<Path>) -> Result<Self> {
        let socket_path = socket_path.as_ref().to_path_buf();

        // Fast path: socket file exists and we can talk to it.
        if socket_path.exists() {
            if let Ok(client) = Self::try_build(&socket_path).await {
                return Ok(client);
            }
            // Socket file exists but daemon may be dead; fall through to
            // auto-start logic.
        }

        // No running daemon detected -- start one.
        info!("Daemon not running, auto-starting...");
        eprintln!("ZLayer daemon not running. Starting...");
        Self::auto_start_daemon(&socket_path).await?;

        Self::try_build(&socket_path)
            .await
            .context("Cannot connect to ZLayer daemon. Run 'zlayer status' for details.")
    }

    /// Like [`connect`](Self::connect) but with a custom TCP endpoint.
    ///
    /// Accepts either `tcp://127.0.0.1:3669` or bare `127.0.0.1:3669`.
    #[cfg(windows)]
    pub async fn connect_to(endpoint: impl Into<String>) -> Result<Self> {
        let raw = endpoint.into();
        let parsed = parse_tcp_url(&raw)?;
        let bearer = read_local_bearer();

        // Fast path: try to talk to an already-running daemon.
        if let Ok(client) = Self::try_build_windows(parsed, bearer.clone()).await {
            return Ok(client);
        }

        // No running daemon detected -- start one.
        info!("Daemon not running, auto-starting...");
        eprintln!("ZLayer daemon not running. Starting...");
        Self::auto_start_daemon_windows(parsed).await?;

        Self::try_build_windows(parsed, bearer)
            .await
            .context("Cannot connect to ZLayer daemon. Run 'zlayer status' for details.")
    }

    /// Build the client and verify that the daemon is reachable via a health
    /// check.  Returns `Err` if the health probe fails.
    #[cfg(unix)]
    async fn try_build(socket_path: &Path) -> Result<Self> {
        let connector = UnixConnector::new(socket_path);
        let client = Client::builder(TokioExecutor::new()).build(connector);

        let dc = Self {
            client,
            socket_path: socket_path.to_path_buf(),
        };

        // Quick health probe -- if this fails the daemon is not (yet) ready.
        if dc.health_check().await? {
            Ok(dc)
        } else {
            bail!("Daemon health check returned unhealthy")
        }
    }

    /// Build the Windows client and verify reachability via a health check.
    #[cfg(windows)]
    async fn try_build_windows(endpoint: SocketAddr, bearer: Option<String>) -> Result<Self> {
        let connector = HttpConnector::new();
        let client = Client::builder(TokioExecutor::new()).build(connector);

        let dc = Self {
            client,
            endpoint,
            bearer,
        };

        if dc.health_check().await? {
            Ok(dc)
        } else {
            bail!("Daemon health check returned unhealthy")
        }
    }

    // ------------------------------------------------------------------
    // Auto-start
    // ------------------------------------------------------------------

    /// Resolve the path to the current executable (since zlayer is now a
    /// single binary, we just re-invoke ourselves).
    fn find_self_binary() -> Result<PathBuf> {
        std::env::current_exe().context("Failed to resolve current executable path")
    }

    /// Spawn `zlayer serve --daemon` and poll the socket with
    /// exponential backoff until the daemon is reachable or we give up.
    #[cfg(unix)]
    async fn auto_start_daemon(socket_path: &Path) -> Result<()> {
        let runtime_bin = Self::find_self_binary()?;

        debug!(binary = %runtime_bin.display(), "Spawning daemon");

        let mut cmd = std::process::Command::new(&runtime_bin);

        // Ensure the child process knows the correct data directory.
        // If ZLAYER_DATA_DIR is already set in the environment it will be
        // inherited automatically (clap picks it up via `env = "ZLAYER_DATA_DIR"`).
        // Otherwise, explicitly pass --data-dir so the child doesn't fall back
        // to /var/lib/zlayer when $HOME is unavailable (e.g. launchd context).
        if std::env::var_os("ZLAYER_DATA_DIR").is_none() {
            let data_dir = zlayer_paths::ZLayerDirs::detect_data_dir();
            cmd.arg("--data-dir").arg(&data_dir);
        }

        cmd.env("ZLAYER_SPAWNER_PID", std::process::id().to_string());

        cmd.arg("serve")
            .arg("--daemon")
            .arg("--socket")
            .arg(socket_path.as_os_str());

        let status = cmd
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .with_context(|| {
                format!("Failed to spawn daemon process: {}", runtime_bin.display())
            })?;

        // The `--daemon` flag causes fork()+setsid() so the parent returns
        // immediately with exit code 0 while the child continues in the
        // background.  If the parent itself fails, that's a real error.
        if !status.success() {
            bail!(
                "Daemon process exited with status {} (binary: {})",
                status,
                runtime_bin.display()
            );
        }

        // Poll socket with exponential backoff: 100ms, 200ms, 400ms, 800ms,
        // then 1s for the remaining attempts.  Max 20 attempts (~10 s).
        let mut delay = Duration::from_millis(100);
        let max_delay = Duration::from_secs(1);
        let max_attempts = 20u32;

        for attempt in 1..=max_attempts {
            tokio::time::sleep(delay).await;

            if socket_path.exists() {
                // Try connecting to confirm the daemon is accepting connections.
                match UnixStream::connect(socket_path).await {
                    Ok(_) => {
                        info!(attempts = attempt, "Daemon socket is accepting connections");
                        return Ok(());
                    }
                    Err(e) => {
                        debug!(
                            attempt,
                            error = %e,
                            "Socket exists but not yet accepting connections"
                        );
                    }
                }
            } else {
                debug!(attempt, "Waiting for daemon socket to appear...");
            }

            delay = std::cmp::min(delay * 2, max_delay);
        }

        // The daemon was spawned with `--data-dir <detect_data_dir>` (or
        // inherited ZLAYER_DATA_DIR), so derive its log dir the same way to
        // surface the correct path under custom data-dirs.
        let spawn_data_dir = std::env::var_os("ZLAYER_DATA_DIR").map_or_else(
            zlayer_paths::ZLayerDirs::detect_data_dir,
            std::path::PathBuf::from,
        );
        let log_dir = zlayer_paths::ZLayerDirs::default_log_dir_for(&spawn_data_dir);
        let log_path = log_dir.join("daemon.log");
        let timeout_secs = 10;
        eprintln!("Failed to start ZLayer daemon after {timeout_secs}s.");
        eprintln!("  Check logs: {}", log_path.display());
        eprintln!("  Start manually: zlayer serve");
        bail!(
            "Timed out waiting for daemon to start after {} attempts (~{} s). \
             Socket path: {}",
            max_attempts,
            timeout_secs,
            socket_path.display()
        )
    }

    /// Windows auto-start: spawn `zlayer serve --daemon --bind <addr>` and
    /// poll the TCP endpoint with exponential backoff.
    #[cfg(windows)]
    async fn auto_start_daemon_windows(endpoint: SocketAddr) -> Result<()> {
        let runtime_bin = Self::find_self_binary()?;

        debug!(binary = %runtime_bin.display(), "Spawning daemon");

        let mut cmd = std::process::Command::new(&runtime_bin);

        // Propagate data-dir in the same way as the Unix path.
        if std::env::var_os("ZLAYER_DATA_DIR").is_none() {
            let data_dir = zlayer_paths::ZLayerDirs::detect_data_dir();
            cmd.arg("--data-dir").arg(&data_dir);
        }

        cmd.env("ZLAYER_SPAWNER_PID", std::process::id().to_string());

        cmd.arg("serve")
            .arg("--daemon")
            .arg("--bind")
            .arg(endpoint.to_string());

        let status = cmd
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .with_context(|| {
                format!("Failed to spawn daemon process: {}", runtime_bin.display())
            })?;

        if !status.success() {
            bail!(
                "Daemon process exited with status {} (binary: {})",
                status,
                runtime_bin.display()
            );
        }

        // Poll endpoint with exponential backoff: 100ms, 200ms, 400ms, 800ms,
        // then 1s for the remaining attempts.  Max 20 attempts (~10 s).
        let mut delay = Duration::from_millis(100);
        let max_delay = Duration::from_secs(1);
        let max_attempts = 20u32;

        for attempt in 1..=max_attempts {
            tokio::time::sleep(delay).await;

            match tokio::net::TcpStream::connect(endpoint).await {
                Ok(_) => {
                    info!(
                        attempts = attempt,
                        "Daemon endpoint is accepting connections"
                    );
                    return Ok(());
                }
                Err(e) => {
                    debug!(
                        attempt,
                        error = %e,
                        "Endpoint not yet accepting connections"
                    );
                }
            }

            delay = std::cmp::min(delay * 2, max_delay);
        }

        // Mirror the data-dir resolution used when spawning the daemon so the
        // hint points at the correct logs under custom data-dirs.
        let spawn_data_dir = std::env::var_os("ZLAYER_DATA_DIR").map_or_else(
            zlayer_paths::ZLayerDirs::detect_data_dir,
            std::path::PathBuf::from,
        );
        let log_dir = zlayer_paths::ZLayerDirs::default_log_dir_for(&spawn_data_dir);
        let log_path = log_dir.join("daemon.log");
        let timeout_secs = 10;
        eprintln!("Failed to start ZLayer daemon after {timeout_secs}s.");
        eprintln!("  Check logs: {}", log_path.display());
        eprintln!("  Start manually: zlayer serve");
        bail!(
            "Timed out waiting for daemon to start after {max_attempts} attempts (~{timeout_secs} s). \
             Endpoint: {endpoint}"
        )
    }

    // ------------------------------------------------------------------
    // Low-level HTTP helpers
    // ------------------------------------------------------------------

    /// Build the request URI for a given API path.
    ///
    /// On Unix the host is the literal `localhost` (ignored by
    /// [`UnixConnector`], which always dials the configured socket). On
    /// Windows the host is the configured TCP endpoint so the stock
    /// [`HttpConnector`] can resolve and dial it.
    #[cfg_attr(unix, allow(clippy::unused_self))]
    fn uri(&self, path: &str) -> Result<Uri> {
        #[cfg(unix)]
        let url = format!("http://localhost{path}");
        #[cfg(windows)]
        let url = format!("http://{}{path}", self.endpoint);

        url.parse::<Uri>()
            .map_err(|e| anyhow::anyhow!("invalid uri {url:?}: {e}"))
    }

    /// Attach a Bearer token to outbound CLI requests when one is appropriate
    /// for the current transport.
    ///
    /// **On Unix (UDS transport):** This is a no-op. The CLI on Unix always
    /// talks to the local daemon over its Unix domain socket, and the daemon
    /// installs a middleware that injects an admin Bearer for any request
    /// lacking `Authorization` (see `crates/zlayer-api/src/server.rs` —
    /// `bind_dual_with_local_auth` + the `unix_router.layer(...)` call in
    /// `serve_bound`). Reading `~/.zlayer/session.json` and attaching its
    /// token would shadow that injection with whatever the file happens to
    /// contain — including a token signed under a previous JWT secret, which
    /// the daemon would reject as `InvalidSignature`. The local CLI never
    /// logs in. Period.
    ///
    /// **On Windows (TCP loopback):** Prefer a saved session bearer from
    /// `~/.zlayer/session.json` when present and unexpired; otherwise fall
    /// back to the locally-persisted admin bearer (`self.bearer`, populated
    /// from `default_admin_bearer_path()` at connect time).
    #[cfg_attr(unix, allow(clippy::unused_self))]
    fn apply_session_auth(
        &self,
        builder: hyper::http::request::Builder,
    ) -> hyper::http::request::Builder {
        #[cfg(unix)]
        {
            // Trust the daemon's UDS middleware; do not attach a session
            // bearer that could be stale and override the middleware's
            // freshly-minted local-admin token.
            builder
        }

        #[cfg(windows)]
        {
            let mut builder = builder;
            let mut session_attached = false;
            match crate::session::read_session() {
                Ok(Some(session)) if !session.is_expired() => {
                    builder = builder.header(
                        hyper::header::AUTHORIZATION,
                        format!("Bearer {}", session.token),
                    );
                    session_attached = true;
                }
                Ok(Some(expired)) => {
                    tracing::debug!(
                        email = %expired.email,
                        "Session file present but token has expired; ignoring"
                    );
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Failed to read session file; proceeding without bearer"
                    );
                }
            }

            if !session_attached {
                if let Some(bearer) = self.bearer.as_deref() {
                    builder =
                        builder.header(hyper::header::AUTHORIZATION, format!("Bearer {bearer}"));
                }
            }

            builder
        }
    }

    /// Send a GET request and return the response body as bytes.
    async fn get(&self, path: &str) -> Result<(hyper::StatusCode, Bytes)> {
        let uri = self.uri(path)?;

        let builder = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri(uri)
            .header("Host", "localhost");
        let builder = self.apply_session_auth(builder);
        let req = builder
            .body(Full::new(Bytes::new()))
            .context("Failed to build GET request")?;

        let resp = self
            .client
            .request(req)
            .await
            .with_context(|| format!("GET {path} failed"))?;

        let status = resp.status();
        let body = resp
            .into_body()
            .collect()
            .await
            .context("Failed to read response body")?
            .to_bytes();

        Ok((status, body))
    }

    /// Send a POST request with a JSON body and return the response.
    async fn post_json(&self, path: &str, body: &str) -> Result<(hyper::StatusCode, Bytes)> {
        let uri = self.uri(path)?;

        let builder = hyper::Request::builder()
            .method(hyper::Method::POST)
            .uri(uri)
            .header("Host", "localhost")
            .header("Content-Type", "application/json");
        let builder = self.apply_session_auth(builder);
        let req = builder
            .body(Full::new(Bytes::from(body.to_owned())))
            .context("Failed to build POST request")?;

        let resp = self
            .client
            .request(req)
            .await
            .with_context(|| format!("POST {path} failed"))?;

        let status = resp.status();
        let body = resp
            .into_body()
            .collect()
            .await
            .context("Failed to read response body")?
            .to_bytes();

        Ok((status, body))
    }

    /// Send a DELETE request and return the response.
    async fn delete(&self, path: &str) -> Result<(hyper::StatusCode, Bytes)> {
        let uri = self.uri(path)?;

        let builder = hyper::Request::builder()
            .method(hyper::Method::DELETE)
            .uri(uri)
            .header("Host", "localhost");
        let builder = self.apply_session_auth(builder);
        let req = builder
            .body(Full::new(Bytes::new()))
            .context("Failed to build DELETE request")?;

        let resp = self
            .client
            .request(req)
            .await
            .with_context(|| format!("DELETE {path} failed"))?;

        let status = resp.status();
        let body = resp
            .into_body()
            .collect()
            .await
            .context("Failed to read response body")?
            .to_bytes();

        Ok((status, body))
    }

    /// Send a POST request with an arbitrary `Content-Type` and raw byte
    /// body. Used by image-load / image-import where the payload is a tar
    /// archive, not JSON.
    async fn post_bytes(
        &self,
        path: &str,
        content_type: &'static str,
        body: Bytes,
    ) -> Result<(hyper::StatusCode, Bytes)> {
        let uri = self.uri(path)?;

        let builder = hyper::Request::builder()
            .method(hyper::Method::POST)
            .uri(uri)
            .header("Host", "localhost")
            .header("Content-Type", content_type);
        let builder = self.apply_session_auth(builder);
        let req = builder
            .body(Full::new(body))
            .context("Failed to build POST request")?;

        let resp = self
            .client
            .request(req)
            .await
            .with_context(|| format!("POST {path} (bytes) failed"))?;

        let status = resp.status();
        let body = resp
            .into_body()
            .collect()
            .await
            .context("Failed to read response body")?
            .to_bytes();
        Ok((status, body))
    }

    /// Send a GET request and stream the response body as raw bytes.
    /// Used by image-save / container-export where the payload is a tar
    /// archive that must not be buffered fully in memory.
    async fn get_stream(
        &self,
        path: &str,
    ) -> Result<(
        hyper::StatusCode,
        std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<Bytes>> + Send>>,
    )> {
        use futures_util::StreamExt;
        use http_body_util::BodyStream;

        let uri = self.uri(path)?;

        let builder = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri(uri)
            .header("Host", "localhost");
        let builder = self.apply_session_auth(builder);
        let req = builder
            .body(Full::new(Bytes::new()))
            .context("Failed to build GET (stream) request")?;

        let resp = self
            .client
            .request(req)
            .await
            .with_context(|| format!("GET {path} (stream) failed"))?;

        let (parts, body) = resp.into_parts();
        let status = parts.status;
        if !status.is_success() {
            let collected = body
                .collect()
                .await
                .context("Failed to read error body")?
                .to_bytes();
            Self::check_status(status, &collected)?;
            unreachable!();
        }

        let frames = BodyStream::new(body);
        let mapped = frames.filter_map(|res| async move {
            match res {
                Ok(frame) => frame.into_data().ok().map(Ok),
                Err(e) => Some(Err(anyhow::anyhow!("stream error: {e}"))),
            }
        });
        Ok((status, Box::pin(mapped)))
    }

    /// Send a POST request with a plain-text body and return the response.
    ///
    /// Used for endpoints whose body is not JSON (e.g. dotenv bulk imports).
    async fn post_text(&self, path: &str, body: &str) -> Result<(hyper::StatusCode, Bytes)> {
        let uri = self.uri(path)?;

        let builder = hyper::Request::builder()
            .method(hyper::Method::POST)
            .uri(uri)
            .header("Host", "localhost")
            .header("Content-Type", "text/plain; charset=utf-8");
        let builder = self.apply_session_auth(builder);
        let req = builder
            .body(Full::new(Bytes::from(body.to_owned())))
            .context("Failed to build POST request")?;

        let resp = self
            .client
            .request(req)
            .await
            .with_context(|| format!("POST {path} failed"))?;

        let status = resp.status();
        let body = resp
            .into_body()
            .collect()
            .await
            .context("Failed to read response body")?
            .to_bytes();

        Ok((status, body))
    }

    /// Send a PUT request with a raw byte body and a custom content type.
    ///
    /// Used by the container-archive endpoints, which speak `application/x-tar`
    /// directly rather than going through the JSON wrapper helpers.
    async fn put_bytes(
        &self,
        path: &str,
        body: Bytes,
        content_type: &str,
    ) -> Result<(hyper::StatusCode, Bytes)> {
        let uri = self.uri(path)?;
        let builder = hyper::Request::builder()
            .method(hyper::Method::PUT)
            .uri(uri)
            .header("Host", "localhost")
            .header("Content-Type", content_type);
        let builder = self.apply_session_auth(builder);
        let req = builder
            .body(Full::new(body))
            .context("Failed to build PUT request")?;
        let resp = self
            .client
            .request(req)
            .await
            .with_context(|| format!("PUT {path} failed"))?;
        let status = resp.status();
        let body = resp
            .into_body()
            .collect()
            .await
            .context("Failed to read response body")?
            .to_bytes();
        Ok((status, body))
    }

    /// Send a HEAD request and return the response status plus the response
    /// headers we care about (currently `X-Docker-Container-Path-Stat`).
    async fn head(&self, path: &str) -> Result<(hyper::StatusCode, Option<String>)> {
        let uri = self.uri(path)?;
        let builder = hyper::Request::builder()
            .method(hyper::Method::HEAD)
            .uri(uri)
            .header("Host", "localhost");
        let builder = self.apply_session_auth(builder);
        let req = builder
            .body(Full::new(Bytes::new()))
            .context("Failed to build HEAD request")?;
        let resp = self
            .client
            .request(req)
            .await
            .with_context(|| format!("HEAD {path} failed"))?;
        let status = resp.status();
        let header = resp
            .headers()
            .get("X-Docker-Container-Path-Stat")
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        Ok((status, header))
    }

    /// Send a PATCH request with a JSON body and return the response.
    async fn patch_json(&self, path: &str, body: &str) -> Result<(hyper::StatusCode, Bytes)> {
        let uri = self.uri(path)?;

        let builder = hyper::Request::builder()
            .method(hyper::Method::PATCH)
            .uri(uri)
            .header("Host", "localhost")
            .header("Content-Type", "application/json");
        let builder = self.apply_session_auth(builder);
        let req = builder
            .body(Full::new(Bytes::from(body.to_owned())))
            .context("Failed to build PATCH request")?;

        let resp = self
            .client
            .request(req)
            .await
            .with_context(|| format!("PATCH {path} failed"))?;

        let status = resp.status();
        let body = resp
            .into_body()
            .collect()
            .await
            .context("Failed to read response body")?
            .to_bytes();

        Ok((status, body))
    }

    /// Parse a JSON response body, returning a contextual error on failure.
    fn parse_json<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T> {
        serde_json::from_slice(body).context("Failed to parse JSON response from daemon")
    }

    /// Decode a response body as a UTF-8 `String`, returning a contextual
    /// error on invalid UTF-8.  Used for endpoints (such as container logs)
    /// that return plain text rather than JSON.
    fn decode_text_body(body: &[u8]) -> Result<String> {
        String::from_utf8(body.to_vec()).context("Container logs body was not valid UTF-8")
    }

    /// Check that a response has a successful (2xx) status code.  If not,
    /// extract the error message from the body and return it with actionable
    /// guidance.
    fn check_status(status: hyper::StatusCode, body: &[u8]) -> Result<()> {
        if status.is_success() {
            return Ok(());
        }

        // Try to extract an error message from the JSON body.
        let msg = if let Ok(val) = serde_json::from_slice::<serde_json::Value>(body) {
            val.get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("unknown error")
                .to_string()
        } else {
            String::from_utf8_lossy(body).into_owned()
        };

        match status.as_u16() {
            404 => bail!("404 Not Found: {msg}. Check deployment name with 'zlayer ps'."),
            500 => {
                // Mirror the data-dir resolution the running daemon would use
                // so the hint is correct under custom `--data-dir`.
                let spawn_data_dir = std::env::var_os("ZLAYER_DATA_DIR").map_or_else(
                    zlayer_paths::ZLayerDirs::detect_data_dir,
                    std::path::PathBuf::from,
                );
                let log_dir = zlayer_paths::ZLayerDirs::default_log_dir_for(&spawn_data_dir);
                let log_path = log_dir.join("daemon.log");
                bail!(
                    "500 Internal Server Error: {}. Check logs at {}",
                    msg,
                    log_path.display()
                )
            }
            _ => bail!("Daemon returned {status} -- {msg}"),
        }
    }

    // ------------------------------------------------------------------
    // Typed API methods
    // ------------------------------------------------------------------

    /// Check if the daemon is alive and healthy.
    ///
    /// Hits `GET /health/live`.
    pub async fn health_check(&self) -> Result<bool> {
        match self.get("/health/live").await {
            Ok((status, _body)) => Ok(status.is_success()),
            // Connection refused / broken pipe => daemon is not up.
            Err(_) => Ok(false),
        }
    }

    /// Check if the daemon is ready (fully initialized).
    ///
    /// Hits `GET /health/ready` and returns the parsed response.
    pub async fn health_ready(&self) -> Result<serde_json::Value> {
        let (status, body) = self.get("/health/ready").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a new deployment from a YAML spec string.
    ///
    /// `POST /api/v1/deployments` with `{"spec": "<yaml>"}`
    pub async fn create_deployment(&self, spec_yaml: &str) -> Result<serde_json::Value> {
        let body = serde_json::json!({ "spec": spec_yaml }).to_string();
        let (status, resp) = self.post_json("/api/v1/deployments", &body).await?;
        // 201 Created is the expected success code.
        if !status.is_success() {
            Self::check_status(status, &resp)?;
        }
        Self::parse_json(&resp)
    }

    /// Get deployment details by name.
    ///
    /// `GET /api/v1/deployments/{name}`
    pub async fn get_deployment(&self, name: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/deployments/{}", urlencoding(name));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Get the stored deployment, including the full `DeploymentSpec`.
    ///
    /// `GET /api/v1/deployments/{name}/spec`
    ///
    /// Unlike [`Self::get_deployment`], which returns the projection used by
    /// the management UI, this fetches the raw stored representation so
    /// callers (notably the Docker Engine API compatibility shim) can read
    /// the original spec — image, command, env, scale, etc. — back out.
    pub async fn get_deployment_stored(&self, name: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/deployments/{}/spec", urlencoding(name));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Watch deployment orchestration progress via SSE.
    ///
    /// `GET /api/v1/deployments/{name}/events`
    ///
    /// Returns a receiver that yields `(event_type, data_json)` tuples as SSE
    /// events arrive. The receiver closes when the server ends the stream
    /// (deployment reached terminal state) or the connection drops.
    pub async fn watch_deployment(
        &self,
        name: &str,
    ) -> Result<tokio::sync::mpsc::Receiver<(String, String)>> {
        let path = format!("/api/v1/deployments/{}/events", urlencoding(name));

        let uri = self.uri(&path)?;

        let req = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri(uri)
            .header("Host", "localhost")
            .header("Accept", "text/event-stream")
            .body(Full::new(Bytes::new()))
            .context("Failed to build GET request for deployment event streaming")?;

        let resp = self
            .client
            .request(req)
            .await
            .with_context(|| format!("GET {path} (SSE) failed"))?;

        let (parts, body) = resp.into_parts();

        if !parts.status.is_success() {
            let collected = body
                .collect()
                .await
                .context("Failed to read error response body")?
                .to_bytes();
            Self::check_status(parts.status, &collected)?;
            unreachable!();
        }

        // Spawn a task that parses SSE frames from the byte stream.
        // Each complete SSE message yields (event_type, data) on the channel.
        let (tx, rx) = tokio::sync::mpsc::channel::<(String, String)>(256);

        tokio::spawn(async move {
            use http_body_util::BodyStream;
            use tokio_stream::StreamExt as _;

            let mut body_stream = BodyStream::new(body);
            let mut buf = String::new();
            let mut current_event = String::new();
            let mut current_data = String::new();

            while let Some(frame_result) = body_stream.next().await {
                let frame = match frame_result {
                    Ok(f) => f,
                    Err(e) => {
                        debug!("SSE body stream error: {}", e);
                        break;
                    }
                };

                if let Some(data) = frame.data_ref() {
                    buf.push_str(&String::from_utf8_lossy(data));

                    // Process complete lines from buffer
                    while let Some(pos) = buf.find('\n') {
                        let line = buf[..pos].to_string();
                        buf.drain(..=pos);

                        if line.is_empty() {
                            // Blank line = end of SSE message
                            if !current_data.is_empty() {
                                let event_type = if current_event.is_empty() {
                                    "message".to_string()
                                } else {
                                    std::mem::take(&mut current_event)
                                };
                                let data = std::mem::take(&mut current_data);
                                if tx.send((event_type, data)).await.is_err() {
                                    return; // receiver dropped
                                }
                            }
                            current_event.clear();
                            current_data.clear();
                        } else if let Some(value) = line.strip_prefix("event:") {
                            current_event = value.trim().to_string();
                        } else if let Some(value) = line.strip_prefix("data:") {
                            current_data = value.trim_start().to_string();
                        }
                        // Ignore other SSE fields (id:, retry:, comments)
                    }
                }
            }

            // Flush any remaining partial message
            if !current_data.is_empty() {
                let event_type = if current_event.is_empty() {
                    "message".to_string()
                } else {
                    current_event
                };
                let _ = tx.send((event_type, current_data)).await;
            }
        });

        Ok(rx)
    }

    /// Delete a deployment by name.
    ///
    /// `DELETE /api/v1/deployments/{name}` -- returns 204 No Content on success.
    pub async fn delete_deployment(&self, name: &str) -> Result<()> {
        let path = format!("/api/v1/deployments/{}", urlencoding(name));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// List all deployments.
    ///
    /// `GET /api/v1/deployments`
    pub async fn list_deployments(&self) -> Result<Vec<serde_json::Value>> {
        let (status, body) = self.get("/api/v1/deployments").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// List services in a deployment.
    ///
    /// `GET /api/v1/deployments/{deployment}/services`
    pub async fn list_services(&self, deployment: &str) -> Result<Vec<serde_json::Value>> {
        let path = format!("/api/v1/deployments/{}/services", urlencoding(deployment));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Get details for a specific service.
    ///
    /// `GET /api/v1/deployments/{deployment}/services/{service}`
    pub async fn get_service(&self, deployment: &str, service: &str) -> Result<serde_json::Value> {
        let path = format!(
            "/api/v1/deployments/{}/services/{}",
            urlencoding(deployment),
            urlencoding(service),
        );
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Scale a service to a specific replica count.
    ///
    /// `POST /api/v1/deployments/{deployment}/services/{service}/scale`
    pub async fn scale_service(
        &self,
        deployment: &str,
        service: &str,
        replicas: u32,
    ) -> Result<serde_json::Value> {
        let path = format!(
            "/api/v1/deployments/{}/services/{}/scale",
            urlencoding(deployment),
            urlencoding(service),
        );
        let body = serde_json::json!({ "replicas": replicas }).to_string();
        let (status, resp) = self.post_json(&path, &body).await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    /// Get logs for a service (non-streaming).
    ///
    /// `GET /api/v1/deployments/{deployment}/services/{service}/logs?lines=N&follow=false[&instance=ID]`
    pub async fn get_logs(
        &self,
        deployment: &str,
        service: &str,
        lines: u32,
        follow: bool,
        instance: Option<&str>,
    ) -> Result<String> {
        self.get_logs_with_instance(deployment, service, lines, follow, instance)
            .await
    }

    /// Get logs for a service, optionally filtered to a specific instance.
    ///
    /// `GET /api/v1/deployments/{deployment}/services/{service}/logs?lines=N&follow=BOOL&instance=ID`
    pub async fn get_logs_with_instance(
        &self,
        deployment: &str,
        service: &str,
        lines: u32,
        follow: bool,
        instance: Option<&str>,
    ) -> Result<String> {
        let mut path = format!(
            "/api/v1/deployments/{}/services/{}/logs?lines={}&follow={}",
            urlencoding(deployment),
            urlencoding(service),
            lines,
            follow,
        );
        if let Some(inst) = instance {
            use std::fmt::Write;
            let _ = write!(path, "&instance={}", urlencoding(inst));
        }
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Ok(String::from_utf8_lossy(&body).into_owned())
    }

    /// Stream logs for a service via SSE (Server-Sent Events).
    ///
    /// Makes a GET request with `follow=true` and returns a receiver that
    /// yields log lines as they arrive.  The caller should read from the
    /// returned `mpsc::Receiver<String>` until it closes (stream ended or
    /// connection dropped).
    ///
    /// `GET /api/v1/deployments/{deployment}/services/{service}/logs?lines=N&follow=true[&instance=ID]`
    pub async fn get_logs_streaming(
        &self,
        deployment: &str,
        service: &str,
        lines: u32,
        instance: Option<&str>,
    ) -> Result<tokio::sync::mpsc::Receiver<String>> {
        let mut path = format!(
            "/api/v1/deployments/{}/services/{}/logs?lines={}&follow=true",
            urlencoding(deployment),
            urlencoding(service),
            lines,
        );
        if let Some(inst) = instance {
            use std::fmt::Write;
            let _ = write!(path, "&instance={}", urlencoding(inst));
        }

        let uri = self.uri(&path)?;

        let req = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri(uri)
            .header("Host", "localhost")
            .header("Accept", "text/event-stream")
            .body(Full::new(Bytes::new()))
            .context("Failed to build GET request for log streaming")?;

        let resp = self
            .client
            .request(req)
            .await
            .with_context(|| format!("GET {path} (streaming) failed"))?;

        let (parts, body) = resp.into_parts();

        if !parts.status.is_success() {
            let collected = body
                .collect()
                .await
                .context("Failed to read error response body")?
                .to_bytes();
            Self::check_status(parts.status, &collected)?;
            // check_status always bails on non-success, but satisfy the compiler:
            unreachable!();
        }

        // Spawn a task to read the SSE byte stream and parse `data:` lines
        let (tx, rx) = tokio::sync::mpsc::channel::<String>(256);

        tokio::spawn(async move {
            use http_body_util::BodyStream;
            use tokio_stream::StreamExt as _;

            let mut body_stream = BodyStream::new(body);
            let mut buf = String::new();

            while let Some(frame_result) = body_stream.next().await {
                let frame = match frame_result {
                    Ok(f) => f,
                    Err(e) => {
                        debug!("SSE body stream error: {}", e);
                        break;
                    }
                };

                if let Some(data) = frame.data_ref() {
                    buf.push_str(&String::from_utf8_lossy(data));

                    // Process complete SSE lines from buffer
                    while let Some(pos) = buf.find('\n') {
                        let line = buf[..pos].to_string();
                        buf.drain(..=pos);

                        // SSE `data:` prefix
                        if let Some(payload) = line.strip_prefix("data:") {
                            let payload = payload.trim_start().to_string();
                            if tx.send(payload).await.is_err() {
                                // Receiver dropped
                                return;
                            }
                        }
                        // Ignore other SSE fields (event:, id:, retry:, comments)
                    }
                }
            }
        });

        Ok(rx)
    }

    /// List containers for a specific service in a deployment.
    ///
    /// `GET /api/v1/deployments/{deployment}/services/{service}/containers`
    pub async fn list_containers(
        &self,
        deployment: &str,
        service: &str,
    ) -> Result<Vec<serde_json::Value>> {
        let path = format!(
            "/api/v1/deployments/{}/services/{}/containers",
            urlencoding(deployment),
            urlencoding(service),
        );
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Execute a command in a service container.
    ///
    /// `POST /api/v1/deployments/{deployment}/services/{service}/exec`
    /// with `{"command": [...], "replica": N}` (replica is optional).
    ///
    /// # Arguments
    /// * `deployment` - Deployment name
    /// * `service` - Service name
    /// * `cmd` - Command and arguments to execute
    ///
    /// # Returns
    /// JSON with `exit_code`, `stdout`, and `stderr` fields.
    pub async fn exec_command(
        &self,
        deployment: &str,
        service: &str,
        cmd: &[String],
    ) -> Result<serde_json::Value> {
        self.exec_command_with_replica(deployment, service, cmd, None)
            .await
    }

    /// Execute a command in a specific replica of a service container.
    ///
    /// `POST /api/v1/deployments/{deployment}/services/{service}/exec`
    /// with `{"command": [...], "replica": N}`.
    pub async fn exec_command_with_replica(
        &self,
        deployment: &str,
        service: &str,
        cmd: &[String],
        replica: Option<u32>,
    ) -> Result<serde_json::Value> {
        let path = format!(
            "/api/v1/deployments/{}/services/{}/exec",
            urlencoding(deployment),
            urlencoding(service),
        );
        let mut payload = serde_json::json!({ "command": cmd });
        if let Some(rep) = replica {
            payload["replica"] = serde_json::json!(rep);
        }
        let body = payload.to_string();
        let (status, resp) = self.post_json(&path, &body).await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    /// Get an authentication token from the daemon.
    ///
    /// `POST /auth/token` with `{"api_key": key, "api_secret": secret}`
    pub async fn get_token(&self, api_key: &str, api_secret: &str) -> Result<serde_json::Value> {
        let body = serde_json::json!({
            "api_key": api_key,
            "api_secret": api_secret,
        })
        .to_string();
        let (status, resp) = self.post_json("/auth/token", &body).await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    /// Get the socket path this client is connected to (Unix only).
    #[cfg(unix)]
    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Get the TCP endpoint this client is connected to (Windows only).
    #[cfg(windows)]
    #[must_use]
    pub fn endpoint(&self) -> &SocketAddr {
        &self.endpoint
    }

    // ------------------------------------------------------------------
    // Image management
    // ------------------------------------------------------------------

    /// List cached images from the daemon.
    ///
    /// `GET /api/v1/images`
    pub async fn list_images(&self) -> Result<Vec<ImageInfoDto>> {
        let (status, body) = self.get("/api/v1/images").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Remove a cached image from the daemon.
    ///
    /// `DELETE /api/v1/images/{image}?force=BOOL` -- returns 204 No Content.
    pub async fn remove_image(&self, image: &str, force: bool) -> Result<()> {
        let path = format!("/api/v1/images/{}?force={}", urlencoding(image), force);
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// Prune dangling / unused cached images from the daemon.
    ///
    /// `POST /api/v1/system/prune`
    pub async fn prune_images(&self) -> Result<PruneResultDto> {
        // Send an empty JSON object as body to keep Content-Type consistent.
        let (status, body) = self.post_json("/api/v1/system/prune", "{}").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Push an image to a remote registry.
    ///
    /// `POST /api/v1/images/push`
    pub async fn push_image(
        &self,
        image: &str,
        username: Option<&str>,
        password: Option<&str>,
    ) -> Result<serde_json::Value> {
        let payload = serde_json::json!({
            "image": image,
            "username": username,
            "password": password,
        });
        let (status, body) = self
            .post_json("/api/v1/images/push", &payload.to_string())
            .await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Inspect an image — retrieve manifest and configuration details.
    ///
    /// `GET /api/v1/images/{image}/inspect`
    pub async fn inspect_image(&self, image: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/images/{}/inspect", urlencoding(image));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Ask the daemon to pull an OCI image reference into its local cache.
    ///
    /// `POST /api/v1/images/pull` with
    /// `{"reference": "<ref>", "pull_policy": "always"|"if_not_present"|"never"}`.
    ///
    /// `pull_policy` is optional and defaults to `"always"` server-side when
    /// omitted. Returns the canonical reference plus the best-effort digest
    /// and on-disk size reported by the runtime.
    pub async fn pull_image_from_server(
        &self,
        reference: &str,
        pull_policy: Option<&str>,
    ) -> Result<PullImageResponse> {
        let mut payload = serde_json::json!({ "reference": reference });
        if let Some(policy) = pull_policy {
            payload["pull_policy"] = serde_json::json!(policy);
        }
        let (status, body) = self
            .post_json("/api/v1/images/pull", &payload.to_string())
            .await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a new reference (`target`) pointing at an already-cached
    /// image (`source`). Matches Docker-compat `docker tag` semantics.
    ///
    /// `POST /api/v1/images/tag` with `{"source": "<ref>", "target": "<ref>"}`.
    /// Returns `Ok(())` on 204 No Content.
    pub async fn tag_image(&self, source: &str, target: &str) -> Result<()> {
        let payload = serde_json::json!({
            "source": source,
            "target": target,
        });
        let (status, body) = self
            .post_json("/api/v1/images/tag", &payload.to_string())
            .await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// Inspect an image and return Docker-shaped metadata.
    ///
    /// `GET /api/v1/images/{image}/inspect`.
    pub async fn inspect_image_native(&self, image: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/images/{}/inspect", urlencoding(image));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Return the parent-layer history for an image.
    ///
    /// `GET /api/v1/images/{image}/history`.
    pub async fn image_history(&self, image: &str) -> Result<Vec<ImageHistoryEntryDto>> {
        let path = format!("/api/v1/images/{}/history", urlencoding(image));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Search the configured registry for images matching `term`.
    ///
    /// `GET /api/v1/images/search?term=&limit=`.
    pub async fn search_images(&self, term: &str, limit: u32) -> Result<Vec<ImageSearchResultDto>> {
        let path = format!(
            "/api/v1/images/search?term={}&limit={}",
            urlencoding(term),
            limit
        );
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Save one or more images to a tar archive, streamed.
    ///
    /// `GET /api/v1/images/save?names=...`. Returns a stream of TAR bytes.
    pub async fn save_images(
        &self,
        names: &[String],
    ) -> Result<std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<Bytes>> + Send>>> {
        if names.is_empty() {
            anyhow::bail!("save_images requires at least one image name");
        }
        let mut path = String::from("/api/v1/images/save?");
        let mut first = true;
        for name in names {
            if !first {
                path.push('&');
            }
            first = false;
            path.push_str("names=");
            path.push_str(&urlencoding(name));
        }
        let (_status, stream) = self.get_stream(&path).await?;
        Ok(stream)
    }

    /// Load images from a tar archive.
    ///
    /// `POST /api/v1/images/load?quiet=BOOL`. Returns the response body
    /// as concatenated NDJSON `LoadProgress` events.
    pub async fn load_images(&self, tar_bytes: Bytes, quiet: bool) -> Result<Bytes> {
        let path = format!("/api/v1/images/load?quiet={quiet}");
        let (status, body) = self
            .post_bytes(&path, "application/x-tar", tar_bytes)
            .await?;
        Self::check_status(status, &body)?;
        Ok(body)
    }

    /// Import an image from a tar root filesystem.
    ///
    /// `POST /api/v1/images/import?repo=&tag=`.
    pub async fn import_image(
        &self,
        tar_bytes: Bytes,
        repo: Option<&str>,
        tag: Option<&str>,
    ) -> Result<ImportImageResponse> {
        let mut path = String::from("/api/v1/images/import?");
        let mut first = true;
        if let Some(repo) = repo {
            path.push_str("repo=");
            path.push_str(&urlencoding(repo));
            first = false;
        }
        if let Some(tag) = tag {
            if !first {
                path.push('&');
            }
            path.push_str("tag=");
            path.push_str(&urlencoding(tag));
        }
        let (status, body) = self
            .post_bytes(&path, "application/x-tar", tar_bytes)
            .await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Stream a tar of a container's filesystem.
    ///
    /// `GET /api/v1/container-export/{id}`.
    pub async fn export_container(
        &self,
        container_id: &str,
    ) -> Result<std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<Bytes>> + Send>>> {
        let path = format!("/api/v1/container-export/{}", urlencoding(container_id));
        let (_status, stream) = self.get_stream(&path).await?;
        Ok(stream)
    }

    /// Commit a running container to a new image.
    ///
    /// `POST /api/v1/commit`.
    pub async fn commit_container_image(
        &self,
        req: &CommitContainerRequest,
    ) -> Result<CommitContainerResponse> {
        let body =
            serde_json::to_string(req).context("Failed to serialise CommitContainerRequest")?;
        let (status, resp) = self.post_json("/api/v1/commit", &body).await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    // ------------------------------------------------------------------
    // Container management
    // ------------------------------------------------------------------

    /// List all containers from the daemon.
    ///
    /// `GET /api/v1/containers`
    pub async fn get_all_containers(&self) -> Result<serde_json::Value> {
        let (status, body) = self.get("/api/v1/containers").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create (and start) a standalone container from a typed request body.
    ///
    /// `POST /api/v1/containers` with the JSON-serialized
    /// [`CreateContainerRequest`]. Returns a [`CreateContainerResponse`] with
    /// the daemon-assigned container id (and any non-fatal warnings).
    ///
    /// The daemon currently replies with the full
    /// [`zlayer_types::api::containers::ContainerInfo`] document on success;
    /// only the fields named on [`CreateContainerResponse`] are read here, the
    /// rest are ignored. `201 Created` is the expected success status.
    pub async fn create_container(
        &self,
        request: CreateContainerRequest,
    ) -> Result<CreateContainerResponse> {
        let body = serde_json::to_string(&request)
            .context("Failed to serialize CreateContainerRequest")?;
        let (status, resp) = self.post_json("/api/v1/containers", &body).await?;
        // 201 Created is the expected success code.
        if !status.is_success() {
            Self::check_status(status, &resp)?;
        }
        Self::parse_json(&resp)
    }

    /// Get detailed information about a single container.
    ///
    /// `GET /api/v1/containers/{id}`
    pub async fn get_container(&self, id: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/containers/{}", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Delete a container.
    ///
    /// `DELETE /api/v1/containers/{id}?force=BOOL`
    pub async fn delete_container(&self, id: &str, force: bool) -> Result<()> {
        let path = format!("/api/v1/containers/{}?force={}", urlencoding(id), force);
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// Get logs for a container.
    ///
    /// `GET /api/v1/containers/{id}/logs[?tail=N]`
    ///
    /// The daemon returns the non-follow log response as plain UTF-8 text
    /// (entries joined by newlines), so the body is decoded as a `String`
    /// rather than parsed as JSON.
    pub async fn get_container_logs(&self, id: &str, tail: Option<u32>) -> Result<String> {
        let mut path = format!("/api/v1/containers/{}/logs", urlencoding(id));
        if let Some(n) = tail {
            use std::fmt::Write;
            let _ = write!(path, "?tail={n}");
        }
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::decode_text_body(&body)
    }

    /// Get resource statistics for a container.
    ///
    /// `GET /api/v1/containers/{id}/stats`
    pub async fn get_container_stats(&self, id: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/containers/{}/stats", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Stop a running standalone container.
    ///
    /// `POST /api/v1/containers/{id}/stop` with `{"timeout": <secs>}`.
    /// `timeout` is the graceful-shutdown window in seconds before the
    /// runtime force-kills the container; the server defaults to 30s when
    /// omitted. Returns `Ok(())` on 204 No Content.
    pub async fn stop_container(&self, id: &str, timeout: Option<u64>) -> Result<()> {
        let path = format!("/api/v1/containers/{}/stop", urlencoding(id));
        let payload = match timeout {
            Some(t) => serde_json::json!({ "timeout": t }),
            None => serde_json::json!({}),
        };
        let (status, body) = self.post_json(&path, &payload.to_string()).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// Start a previously-created standalone container.
    ///
    /// `POST /api/v1/containers/{id}/start` with an empty body.
    /// Returns `Ok(())` on 204 No Content.
    pub async fn start_container(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/containers/{}/start", urlencoding(id));
        let (status, body) = self.post_json(&path, "{}").await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// Restart a standalone container (stop then start).
    ///
    /// `POST /api/v1/containers/{id}/restart` with `{"timeout": <secs>}`.
    /// `timeout` is the graceful-shutdown window in seconds before force-kill;
    /// the server defaults to 30s when omitted. Returns `Ok(())` on 204.
    pub async fn restart_container(&self, id: &str, timeout: Option<u64>) -> Result<()> {
        let path = format!("/api/v1/containers/{}/restart", urlencoding(id));
        let payload = match timeout {
            Some(t) => serde_json::json!({ "timeout": t }),
            None => serde_json::json!({}),
        };
        let (status, body) = self.post_json(&path, &payload.to_string()).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// Send a signal to a running standalone container.
    ///
    /// `POST /api/v1/containers/{id}/kill` with `{"signal": "<name>"}`.
    /// `signal` accepts either the `SIG`-prefixed or bare form (e.g.
    /// `"SIGTERM"` or `"TERM"`). When omitted, the server defaults to
    /// `SIGKILL`. Returns `Ok(())` on 204 No Content.
    pub async fn kill_container(&self, id: &str, signal: Option<&str>) -> Result<()> {
        let path = format!("/api/v1/containers/{}/kill", urlencoding(id));
        let payload = match signal {
            Some(s) => serde_json::json!({ "signal": s }),
            None => serde_json::json!({}),
        };
        let (status, body) = self.post_json(&path, &payload.to_string()).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// Execute a command inside a standalone container and wait for it to
    /// finish.
    ///
    /// `POST /api/v1/containers/{id}/exec` with `{"command": [...]}`.
    /// Returns a [`ContainerExecResponse`] with `exit_code`, `stdout`, and
    /// `stderr`.
    ///
    /// Note: the daemon's container exec endpoint is non-interactive -- it
    /// runs the command to completion and buffers the output. It does not
    /// accept `tty` or `interactive` flags. For attached/TTY exec against a
    /// managed service, use
    /// [`exec_command`](Self::exec_command) instead.
    pub async fn exec_in_container(
        &self,
        id: &str,
        cmd: Vec<String>,
    ) -> Result<ContainerExecResponse> {
        let path = format!("/api/v1/containers/{}/exec", urlencoding(id));
        let payload = serde_json::json!({ "command": cmd });
        let (status, body) = self.post_json(&path, &payload.to_string()).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Wait for a container to reach a terminal state, Docker-style.
    ///
    /// Hits `POST /api/v1/containers/{id}/wait[?condition=<...>]` and returns
    /// the daemon's [`WaitContainerResponse`] (`status_code` + optional
    /// `error.message`).
    ///
    /// `condition` is one of `"not-running"` (default), `"next-exit"`, or
    /// `"removed"` — matching Docker's `/containers/{id}/wait` semantics. When
    /// `None`, the parameter is omitted and the daemon applies its default.
    ///
    /// This client method targets the Docker-compat shape that the
    /// `zlayer-docker` shim drives. The native daemon's
    /// `GET /api/v1/containers/{id}/wait` endpoint (added under §3.12 of the
    /// SDK-fixes spec) returns a richer
    /// [`zlayer_types::api::containers::ContainerWaitResponse`]; a follow-up
    /// daemon sub-task wires the `POST` form alongside it.
    pub async fn wait_container(
        &self,
        id: &str,
        condition: Option<&str>,
    ) -> Result<WaitContainerResponse> {
        let mut path = format!("/api/v1/containers/{}/wait", urlencoding(id));
        if let Some(c) = condition {
            use std::fmt::Write;
            let _ = write!(path, "?condition={}", urlencoding(c));
        }
        let (status, body) = self.post_json(&path, "{}").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Rename a standalone container.
    ///
    /// Hits `POST /api/v1/containers/{id}/rename?name=<new>` and returns
    /// `Ok(())` on the daemon's `204 No Content`.
    ///
    /// Mirrors Docker Engine's `/containers/{id}/rename` endpoint shape so
    /// the `zlayer-docker` compatibility shim can forward the call directly,
    /// and the SDK can expose a Docker-equivalent helper.
    pub async fn rename_container(&self, id: &str, new_name: &str) -> Result<()> {
        let path = format!(
            "/api/v1/containers/{}/rename?name={}",
            urlencoding(id),
            urlencoding(new_name)
        );
        let (status, body) = self.post_json(&path, "{}").await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// Update a standalone container's resource limits and/or restart
    /// policy.
    ///
    /// Hits `POST /api/v1/containers/{id}/update` with the Docker-shaped
    /// JSON body (`CpuShares`, `Memory`, `RestartPolicy`, ...). Returns
    /// the daemon's [`ContainerUpdateResponse`] (`{"Warnings": [...]}`).
    ///
    /// Mirrors Docker Engine's `POST /containers/{id}/update` shape so the
    /// `zlayer-docker` compatibility shim can forward the call directly.
    pub async fn update_container(
        &self,
        id: &str,
        update: &ContainerUpdateRequest,
    ) -> Result<ContainerUpdateResponse> {
        let path = format!("/api/v1/containers/{}/update", urlencoding(id));
        let body =
            serde_json::to_string(update).context("failed to serialize ContainerUpdateRequest")?;
        let (status, resp) = self.post_json(&path, &body).await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    /// Pause a running standalone container.
    ///
    /// `POST /api/v1/containers/{id}/pause`. Returns `Ok(())` on 204.
    pub async fn pause_container(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/containers/{}/pause", urlencoding(id));
        let (status, body) = self.post_json(&path, "{}").await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// Resume a previously-paused standalone container.
    ///
    /// `POST /api/v1/containers/{id}/unpause`. Returns `Ok(())` on 204.
    pub async fn unpause_container(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/containers/{}/unpause", urlencoding(id));
        let (status, body) = self.post_json(&path, "{}").await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// List the processes running inside a standalone container.
    ///
    /// `GET /api/v1/containers/{id}/top?ps_args=<...>`. Returns the daemon's
    /// `ContainerTopResponse` JSON body verbatim. `ps_args` is forwarded as
    /// a single query string when supplied; pass `None` to use the runtime's
    /// default columns.
    pub async fn top_container(
        &self,
        id: &str,
        ps_args: Option<&str>,
    ) -> Result<serde_json::Value> {
        let mut path = format!("/api/v1/containers/{}/top", urlencoding(id));
        if let Some(args) = ps_args {
            use std::fmt::Write;
            let _ = write!(path, "?ps_args={}", urlencoding(args));
        }
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Report changes to a container's filesystem.
    ///
    /// `GET /api/v1/containers/{id}/changes`. Returns the daemon's
    /// `Vec<ContainerChangeEntry>` body (Docker-shaped `Path`/`Kind`).
    pub async fn container_changes(&self, id: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/containers/{}/changes", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Report the published port mappings for a container.
    ///
    /// `GET /api/v1/containers/{id}/port`. Returns the daemon's
    /// `ContainerPortResponse` body (Docker-shaped `{"Ports": ...}`).
    pub async fn container_port(&self, id: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/containers/{}/port", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Prune stopped containers from the runtime.
    ///
    /// `POST /api/v1/containers/prune`. Returns the daemon's
    /// `ContainerPruneResponse` body (`ContainersDeleted` + `SpaceReclaimed`).
    pub async fn prune_standalone_containers(&self) -> Result<serde_json::Value> {
        let path = "/api/v1/containers/prune";
        let (status, body) = self.post_json(path, "{}").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Download a TAR archive of a path inside a container.
    ///
    /// Hits `GET /api/v1/containers/{id}/archive?path=<container_path>` and
    /// returns the raw `application/x-tar` response body. The full archive
    /// is buffered in memory; for large copies callers should pipe the bytes
    /// straight to disk.
    ///
    /// Returns the raw archive bytes plus the optional
    /// `X-Docker-Container-Path-Stat` header value (base64-encoded JSON
    /// describing the archived path).
    pub async fn archive_get(
        &self,
        id: &str,
        container_path: &str,
    ) -> Result<(Bytes, Option<String>)> {
        let path = format!(
            "/api/v1/containers/{}/archive?path={}",
            urlencoding(id),
            urlencoding(container_path)
        );
        // The lightweight `get()` helper drops response headers, so build the
        // request inline here and capture the path-stat header in addition to
        // the body.
        let uri = self.uri(&path)?;
        let builder = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri(uri)
            .header("Host", "localhost");
        let builder = self.apply_session_auth(builder);
        let req = builder
            .body(Full::new(Bytes::new()))
            .context("Failed to build GET request")?;
        let resp = self
            .client
            .request(req)
            .await
            .with_context(|| format!("GET {path} failed"))?;
        let status = resp.status();
        let stat_header = resp
            .headers()
            .get("X-Docker-Container-Path-Stat")
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let body = resp
            .into_body()
            .collect()
            .await
            .context("Failed to read response body")?
            .to_bytes();
        Self::check_status(status, &body)?;
        Ok((body, stat_header))
    }

    /// Upload (extract) a TAR archive into a container at a given path.
    ///
    /// Hits `PUT /api/v1/containers/{id}/archive?path=<container_path>` with
    /// the supplied uncompressed TAR bytes. `no_overwrite_dir_non_dir` and
    /// `copy_uid_gid` mirror Docker's `noOverwriteDirNonDir` /
    /// `copyUIDGID` query parameters.
    pub async fn archive_put(
        &self,
        id: &str,
        container_path: &str,
        tar_bytes: Bytes,
        no_overwrite_dir_non_dir: bool,
        copy_uid_gid: bool,
    ) -> Result<()> {
        use std::fmt::Write;
        let mut path = format!(
            "/api/v1/containers/{}/archive?path={}",
            urlencoding(id),
            urlencoding(container_path)
        );
        if no_overwrite_dir_non_dir {
            let _ = write!(path, "&noOverwriteDirNonDir=1");
        }
        if copy_uid_gid {
            let _ = write!(path, "&copyUIDGID=1");
        }
        let (status, body) = self
            .put_bytes(&path, tar_bytes, "application/x-tar")
            .await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// Stat a path inside a container without materializing its TAR archive.
    ///
    /// Hits `HEAD /api/v1/containers/{id}/archive?path=<container_path>` and
    /// returns the base64-encoded JSON contents of the
    /// `X-Docker-Container-Path-Stat` response header. Returns `None` when
    /// the daemon omits the header (older daemons / transports that strip
    /// custom headers).
    pub async fn archive_head(&self, id: &str, container_path: &str) -> Result<Option<String>> {
        let path = format!(
            "/api/v1/containers/{}/archive?path={}",
            urlencoding(id),
            urlencoding(container_path)
        );
        let (status, header) = self.head(&path).await?;
        if !status.is_success() {
            bail!("HEAD {path} returned {status}");
        }
        Ok(header)
    }

    // ------------------------------------------------------------------
    // Tunnel management
    // ------------------------------------------------------------------

    /// List all tunnels.
    ///
    /// `GET /api/v1/tunnels`
    pub async fn list_tunnels(&self) -> Result<serde_json::Value> {
        let (status, body) = self.get("/api/v1/tunnels").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Revoke (delete) a tunnel by ID.
    ///
    /// `DELETE /api/v1/tunnels/{id}`
    pub async fn revoke_tunnel(&self, id: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/tunnels/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Get tunnel status by ID.
    ///
    /// `GET /api/v1/tunnels/{id}/status`
    pub async fn get_tunnel_status(&self, id: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/tunnels/{}/status", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a node-to-node tunnel.
    ///
    /// `POST /api/v1/tunnels/node`
    pub async fn add_node_tunnel(
        &self,
        name: &str,
        from_node: &str,
        to_node: &str,
        local_port: u16,
        remote_port: u16,
        expose: &str,
    ) -> Result<serde_json::Value> {
        let payload = serde_json::json!({
            "name": name,
            "from_node": from_node,
            "to_node": to_node,
            "local_port": local_port,
            "remote_port": remote_port,
            "expose": expose,
        });
        let (status, body) = self
            .post_json("/api/v1/tunnels/node", &payload.to_string())
            .await?;
        if !status.is_success() {
            Self::check_status(status, &body)?;
        }
        Self::parse_json(&body)
    }

    /// Remove a node-to-node tunnel by name.
    ///
    /// `DELETE /api/v1/tunnels/node/{name}`
    pub async fn remove_node_tunnel(&self, name: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/tunnels/node/{}", urlencoding(name));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a temporary access session for a tunneled service.
    ///
    /// The daemon binds a local TCP listener that proxies to the requested
    /// `endpoint` and tears it down when `ttl_secs` elapses. Returns the
    /// daemon-side local address so the caller can connect (or open a
    /// secondary forwarder when running on a different host).
    ///
    /// `POST /api/v1/tunnels/access/sessions`
    pub async fn create_access_session(
        &self,
        endpoint: &str,
        ttl_secs: u64,
        local_port: Option<u16>,
    ) -> Result<zlayer_types::api::tunnels::CreateAccessSessionResponse> {
        // `CreateAccessSessionRequest` is `Deserialize`-only on the daemon
        // side, so we build the wire JSON inline rather than going through
        // `serde_json::to_string`.
        let mut payload = serde_json::json!({
            "endpoint": endpoint,
            "ttl_secs": ttl_secs,
        });
        if let Some(port) = local_port {
            payload["local_port"] = serde_json::Value::from(port);
        }
        let (status, resp) = self
            .post_json("/api/v1/tunnels/access/sessions", &payload.to_string())
            .await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    // ------------------------------------------------------------------
    // Secrets management
    // ------------------------------------------------------------------

    /// List all secrets from the daemon.
    ///
    /// `GET /api/v1/secrets`
    pub async fn list_secrets(&self) -> Result<Vec<serde_json::Value>> {
        let (status, body) = self.get("/api/v1/secrets").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create or update a secret.
    ///
    /// `POST /api/v1/secrets` with `{"name": "<name>", "value": "<value>"}`
    pub async fn create_secret(&self, name: &str, value: &str) -> Result<serde_json::Value> {
        let payload = serde_json::json!({ "name": name, "value": value });
        let (status, body) = self
            .post_json("/api/v1/secrets", &payload.to_string())
            .await?;
        if !status.is_success() {
            Self::check_status(status, &body)?;
        }
        Self::parse_json(&body)
    }

    /// Get metadata for a specific secret (value is never returned).
    ///
    /// `GET /api/v1/secrets/{name}`
    pub async fn get_secret(&self, name: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/secrets/{}", urlencoding(name));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Delete a secret by name.
    ///
    /// `DELETE /api/v1/secrets/{name}` -- returns 204 No Content on success.
    pub async fn delete_secret(&self, name: &str) -> Result<()> {
        let path = format!("/api/v1/secrets/{}", urlencoding(name));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Network management
    // ------------------------------------------------------------------

    /// List all networks.
    ///
    /// `GET /api/v1/networks`
    pub async fn list_networks(&self) -> Result<Vec<serde_json::Value>> {
        let (status, body) = self.get("/api/v1/networks").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Get details for a specific network by name.
    ///
    /// `GET /api/v1/networks/{name}`
    pub async fn get_network(&self, name: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/networks/{}", urlencoding(name));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a new network.
    ///
    /// `POST /api/v1/networks` with `{"name": "<name>"}`
    pub async fn create_network(&self, name: &str) -> Result<serde_json::Value> {
        let payload = serde_json::json!({ "name": name });
        let (status, body) = self
            .post_json("/api/v1/networks", &payload.to_string())
            .await?;
        if !status.is_success() {
            Self::check_status(status, &body)?;
        }
        Self::parse_json(&body)
    }

    /// Delete a network by name.
    ///
    /// `DELETE /api/v1/networks/{name}` -- returns 204 No Content on success.
    pub async fn delete_network(&self, name: &str) -> Result<()> {
        let path = format!("/api/v1/networks/{}", urlencoding(name));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Volume management
    // ------------------------------------------------------------------

    /// List all volumes.
    ///
    /// `GET /api/v1/volumes`
    pub async fn list_volumes(&self) -> Result<Vec<serde_json::Value>> {
        let (status, body) = self.get("/api/v1/volumes").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Inspect a single volume by name.
    ///
    /// `GET /api/v1/volumes/{name}`. Returns `Ok(None)` when the daemon
    /// reports `404 Not Found`; any other non-success status is bubbled up as
    /// an error. The returned [`VolumeInfo`] matches the daemon-side handler
    /// shape (`zlayer_types::api::volumes::VolumeInfo`).
    pub async fn inspect_volume(
        &self,
        name: &str,
    ) -> Result<Option<zlayer_types::api::volumes::VolumeInfo>> {
        let path = format!("/api/v1/volumes/{}", urlencoding(name));
        let (status, body) = self.get(&path).await?;
        if status == hyper::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Self::check_status(status, &body)?;
        Self::parse_json(&body).map(Some)
    }

    /// Create a new volume.
    ///
    /// `POST /api/v1/volumes` with the JSON-serialized
    /// [`zlayer_types::api::volumes::CreateVolumeRequest`]. Returns the
    /// freshly-created [`VolumeInfo`] document on `201 Created`.
    ///
    /// When the daemon reports `409 Conflict` (a volume with this name
    /// already exists), the method follows up with
    /// [`Self::inspect_volume`] and returns the existing volume. This
    /// mirrors Docker's idempotent `volume create` semantics, which the
    /// `zlayer-docker` compat shim relies on.
    pub async fn create_volume(
        &self,
        request: zlayer_types::api::volumes::CreateVolumeRequest,
    ) -> Result<zlayer_types::api::volumes::VolumeInfo> {
        let name = request.name.clone();
        let body =
            serde_json::to_string(&request).context("Failed to serialize CreateVolumeRequest")?;
        let (status, resp) = self.post_json("/api/v1/volumes", &body).await?;
        if status == hyper::StatusCode::CONFLICT {
            return self.inspect_volume(&name).await?.ok_or_else(|| {
                anyhow::anyhow!(
                    "Daemon reported volume '{name}' already exists, but inspect returned 404"
                )
            });
        }
        if !status.is_success() {
            Self::check_status(status, &resp)?;
        }
        Self::parse_json(&resp)
    }

    /// Delete a volume by name.
    ///
    /// `DELETE /api/v1/volumes/{name}?force={force}` -- returns 204 No Content on success.
    pub async fn delete_volume(&self, name: &str, force: bool) -> Result<()> {
        let path = format!("/api/v1/volumes/{}?force={}", urlencoding(name), force);
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Bridge / overlay container networks (`/api/v1/container-networks`)
    // ------------------------------------------------------------------

    /// List all user-defined bridge / overlay networks.
    ///
    /// `GET /api/v1/container-networks`. Returns the daemon's
    /// `Vec<BridgeNetwork>` ordered by `created_at` then `name`.
    pub async fn list_bridge_networks(&self) -> Result<Vec<zlayer_types::spec::BridgeNetwork>> {
        let (status, body) = self.get("/api/v1/container-networks").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a new bridge / overlay network.
    ///
    /// `POST /api/v1/container-networks` with the JSON-serialized
    /// [`zlayer_types::api::container_networks::CreateBridgeNetworkRequest`].
    /// Returns the daemon-assigned [`BridgeNetwork`] on `201 Created`.
    pub async fn create_bridge_network(
        &self,
        request: zlayer_types::api::container_networks::CreateBridgeNetworkRequest,
    ) -> Result<zlayer_types::spec::BridgeNetwork> {
        let body = serde_json::to_string(&request)
            .context("Failed to serialize CreateBridgeNetworkRequest")?;
        let (status, resp) = self.post_json("/api/v1/container-networks", &body).await?;
        if !status.is_success() {
            Self::check_status(status, &resp)?;
        }
        Self::parse_json(&resp)
    }

    /// Inspect a single bridge / overlay network by id or name.
    ///
    /// `GET /api/v1/container-networks/{id_or_name}`. Returns `Ok(None)` for
    /// `404 Not Found`. The daemon emits a `BridgeNetworkDetails` document
    /// (flattened `BridgeNetwork` + `attached_containers`); this method
    /// deserializes only the [`BridgeNetwork`] portion, matching the shape
    /// callers asked for. Inspect plus attachments lives under a separate
    /// future helper if needed.
    pub async fn get_bridge_network(
        &self,
        id: &str,
    ) -> Result<Option<zlayer_types::spec::BridgeNetwork>> {
        let path = format!("/api/v1/container-networks/{}", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        if status == hyper::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Self::check_status(status, &body)?;
        Self::parse_json(&body).map(Some)
    }

    /// Delete a bridge / overlay network by id or name.
    ///
    /// `DELETE /api/v1/container-networks/{id_or_name}`. Returns `Ok(true)`
    /// on `204 No Content`, `Ok(false)` on `404 Not Found`, and an error for
    /// any other non-success status. The daemon refuses deletion when the
    /// network still has attached containers — pass `force=true` (not
    /// supported by this helper) via a direct request if that override is
    /// required.
    pub async fn delete_bridge_network(&self, id: &str) -> Result<bool> {
        let path = format!("/api/v1/container-networks/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        if status == hyper::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        Self::check_status(status, &body)?;
        Ok(true)
    }

    /// Attach a container to a bridge / overlay network.
    ///
    /// `POST /api/v1/container-networks/{network}/connect` with the JSON
    /// body `{"container_id": "<id>", "aliases": [...]}`. Returns `Ok(())`
    /// on `204 No Content`. The optional `ipv4_address` field of
    /// [`ConnectBridgeNetworkRequest`] is intentionally not exposed by this
    /// helper — callers that need static IP assignment should drop down to
    /// `post_json` directly.
    pub async fn connect_container_to_bridge_network(
        &self,
        network: &str,
        container: &str,
        aliases: Vec<String>,
    ) -> Result<()> {
        let path = format!(
            "/api/v1/container-networks/{}/connect",
            urlencoding(network)
        );
        let request = zlayer_types::api::container_networks::ConnectBridgeNetworkRequest {
            container_id: container.to_string(),
            aliases,
            ipv4_address: None,
        };
        let body = serde_json::to_string(&request)
            .context("Failed to serialize ConnectBridgeNetworkRequest")?;
        let (status, resp) = self.post_json(&path, &body).await?;
        Self::check_status(status, &resp)?;
        Ok(())
    }

    /// Detach a container from a bridge / overlay network.
    ///
    /// `POST /api/v1/container-networks/{network}/disconnect` with the JSON
    /// body `{"container_id": "<id>", "force": <bool>}`. When `force` is
    /// `true`, the daemon silently no-ops on the registry side if the
    /// container was not attached; otherwise an unknown attachment surfaces
    /// as a `404 Not Found`.
    pub async fn disconnect_container_from_bridge_network(
        &self,
        network: &str,
        container: &str,
        force: bool,
    ) -> Result<()> {
        let path = format!(
            "/api/v1/container-networks/{}/disconnect",
            urlencoding(network)
        );
        let request = zlayer_types::api::container_networks::DisconnectBridgeNetworkRequest {
            container_id: container.to_string(),
            force,
        };
        let body = serde_json::to_string(&request)
            .context("Failed to serialize DisconnectBridgeNetworkRequest")?;
        let (status, resp) = self.post_json(&path, &body).await?;
        Self::check_status(status, &resp)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Overlay network
    // ------------------------------------------------------------------

    /// Get overlay network status.
    ///
    /// `GET /api/v1/overlay/status`
    pub async fn get_overlay_status(&self) -> Result<serde_json::Value> {
        let (status, body) = self.get("/api/v1/overlay/status").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Get overlay peer list.
    ///
    /// `GET /api/v1/overlay/peers`
    pub async fn get_overlay_peers(&self) -> Result<serde_json::Value> {
        let (status, body) = self.get("/api/v1/overlay/peers").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Get overlay DNS entries.
    ///
    /// `GET /api/v1/overlay/dns`
    pub async fn get_overlay_dns(&self) -> Result<serde_json::Value> {
        let (status, body) = self.get("/api/v1/overlay/dns").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    // ------------------------------------------------------------------
    // Job management
    // ------------------------------------------------------------------

    /// List all jobs.
    ///
    /// `GET /api/v1/jobs`
    pub async fn list_jobs(&self) -> Result<serde_json::Value> {
        let (status, body) = self.get("/api/v1/jobs").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Trigger a job execution.
    ///
    /// `POST /api/v1/deployments/{deployment}/jobs/{job}/trigger`
    pub async fn trigger_job(&self, deployment: &str, job: &str) -> Result<serde_json::Value> {
        let path = format!(
            "/api/v1/deployments/{}/jobs/{}/trigger",
            urlencoding(deployment),
            urlencoding(job),
        );
        let (status, body) = self.post_json(&path, "{}").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Get job status.
    ///
    /// `GET /api/v1/deployments/{deployment}/jobs/{job}`
    pub async fn get_job_status(&self, deployment: &str, job: &str) -> Result<serde_json::Value> {
        let path = format!(
            "/api/v1/deployments/{}/jobs/{}",
            urlencoding(deployment),
            urlencoding(job),
        );
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// List all cron jobs.
    ///
    /// `GET /api/v1/cron`
    pub async fn list_cron_jobs(&self) -> Result<serde_json::Value> {
        let (status, body) = self.get("/api/v1/cron").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Get cron job status.
    ///
    /// `GET /api/v1/deployments/{deployment}/cron/{cron}`
    pub async fn get_cron_status(&self, deployment: &str, cron: &str) -> Result<serde_json::Value> {
        let path = format!(
            "/api/v1/deployments/{}/cron/{}",
            urlencoding(deployment),
            urlencoding(cron),
        );
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    // ------------------------------------------------------------------
    // Auth
    // ------------------------------------------------------------------

    /// POST /auth/bootstrap -- create the first admin user. Returns the user view and csrf token.
    pub async fn auth_bootstrap(&self, req: &BootstrapRequest) -> Result<LoginResponse> {
        let body = serde_json::to_string(req).context("Failed to serialise BootstrapRequest")?;
        let (status, resp_body) = self.post_json("/auth/bootstrap", &body).await?;
        Self::check_status(status, &resp_body)?;
        Self::parse_json(&resp_body)
    }

    /// POST /auth/login -- authenticate by email+password. Returns the session cookie
    /// via HTTP headers (discarded here -- CLI stores the separately-obtained token from
    /// /auth/token) and the login response body.
    pub async fn auth_login(&self, req: &LoginRequest) -> Result<LoginResponse> {
        let body = serde_json::to_string(req).context("Failed to serialise LoginRequest")?;
        let (status, resp_body) = self.post_json("/auth/login", &body).await?;
        Self::check_status(status, &resp_body)?;
        Self::parse_json(&resp_body)
    }

    /// POST /auth/logout -- invalidate the session cookie on the server.
    /// CLI-side session-file deletion happens separately in commands/auth.rs.
    pub async fn auth_logout(&self) -> Result<()> {
        let (status, resp_body) = self.post_json("/auth/logout", "").await?;
        // 204 No Content is success.
        if status.is_success() {
            Ok(())
        } else {
            Self::check_status(status, &resp_body)
        }
    }

    /// GET /auth/me -- current user from the attached session/Bearer auth.
    pub async fn auth_whoami(&self) -> Result<UserView> {
        let (status, body) = self.get("/auth/me").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// POST /auth/token -- exchange email+password for a JWT. Used by `zlayer auth
    /// login` to persist a token in the session file. Returns a raw JSON value so
    /// the caller can extract `access_token` without coupling to zlayer-api types
    /// that aren't re-exported there.
    pub async fn auth_token(&self, api_key: &str, api_secret: &str) -> Result<serde_json::Value> {
        let body = serde_json::json!({
            "api_key": api_key,
            "api_secret": api_secret,
        })
        .to_string();
        let (status, resp_body) = self.post_json("/auth/token", &body).await?;
        Self::check_status(status, &resp_body)?;
        Self::parse_json(&resp_body)
    }

    // ------------------------------------------------------------------
    // Users
    // ------------------------------------------------------------------

    /// GET /api/v1/users -- list all users (admin).
    pub async fn list_users(&self) -> Result<Vec<UserView>> {
        let (status, body) = self.get("/api/v1/users").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// POST /api/v1/users -- create a new user (admin).
    pub async fn create_user(&self, req: &CreateUserRequest) -> Result<UserView> {
        let body = serde_json::to_string(req).context("Failed to serialise CreateUserRequest")?;
        let (status, resp_body) = self.post_json("/api/v1/users", &body).await?;
        Self::check_status(status, &resp_body)?;
        Self::parse_json(&resp_body)
    }

    /// GET /api/v1/users/{id}.
    pub async fn get_user(&self, id: &str) -> Result<UserView> {
        let path = format!("/api/v1/users/{}", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// PATCH /api/v1/users/{id} -- update user fields.
    pub async fn update_user(&self, id: &str, req: &UpdateUserRequest) -> Result<UserView> {
        let body = serde_json::to_string(req).context("Failed to serialise UpdateUserRequest")?;
        let path = format!("/api/v1/users/{}", urlencoding(id));
        let (status, resp_body) = self.patch_json(&path, &body).await?;
        Self::check_status(status, &resp_body)?;
        Self::parse_json(&resp_body)
    }

    /// DELETE /api/v1/users/{id}.
    pub async fn delete_user(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/users/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        if status.is_success() {
            Ok(())
        } else {
            Self::check_status(status, &body)
        }
    }

    /// POST /api/v1/users/{id}/password -- set a user's password (admin or self-service).
    pub async fn set_user_password(&self, id: &str, req: &SetPasswordRequest) -> Result<()> {
        let body = serde_json::to_string(req).context("Failed to serialise SetPasswordRequest")?;
        let path = format!("/api/v1/users/{}/password", urlencoding(id));
        let (status, resp_body) = self.post_json(&path, &body).await?;
        if status.is_success() {
            Ok(())
        } else {
            Self::check_status(status, &resp_body)
        }
    }

    // ------------------------------------------------------------------
    // Environments
    // ------------------------------------------------------------------

    /// List environments, optionally filtered by project id.
    ///
    /// `GET /api/v1/environments?project={id}` (omit `project` for globals,
    /// pass `*` to list every environment across projects + globals).
    pub async fn list_environments(
        &self,
        project_id: Option<&str>,
    ) -> Result<Vec<StoredEnvironment>> {
        let path = match project_id {
            Some(p) => format!("/api/v1/environments?project={}", urlencoding(p)),
            None => "/api/v1/environments".to_string(),
        };
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a new environment.
    ///
    /// `POST /api/v1/environments` with `{ name, project_id?, description? }`.
    pub async fn create_environment(
        &self,
        name: &str,
        project_id: Option<&str>,
        description: Option<&str>,
    ) -> Result<StoredEnvironment> {
        let body = serde_json::json!({
            "name": name,
            "project_id": project_id,
            "description": description,
        })
        .to_string();
        let (status, resp) = self.post_json("/api/v1/environments", &body).await?;
        if !status.is_success() {
            Self::check_status(status, &resp)?;
        }
        Self::parse_json(&resp)
    }

    /// Fetch a single environment by id.
    ///
    /// `GET /api/v1/environments/{id}`.
    pub async fn get_environment(&self, id: &str) -> Result<StoredEnvironment> {
        let path = format!("/api/v1/environments/{}", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Update an environment's name and/or description.
    ///
    /// `PATCH /api/v1/environments/{id}` with `{ name?, description? }`.
    pub async fn update_environment(
        &self,
        id: &str,
        name: Option<&str>,
        description: Option<&str>,
    ) -> Result<StoredEnvironment> {
        let body = serde_json::json!({
            "name": name,
            "description": description,
        })
        .to_string();
        let path = format!("/api/v1/environments/{}", urlencoding(id));
        let (status, resp) = self.patch_json(&path, &body).await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    /// Delete an environment.
    ///
    /// `DELETE /api/v1/environments/{id}` — 204 on success, 409 if the
    /// environment still has secrets.
    pub async fn delete_environment(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/environments/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Variables
    // ------------------------------------------------------------------

    /// List variables, optionally filtered by scope.
    ///
    /// `GET /api/v1/variables?scope={id}` (omit `scope` for globals).
    pub async fn list_variables(&self, scope: Option<&str>) -> Result<Vec<StoredVariable>> {
        let path = match scope {
            Some(s) => format!("/api/v1/variables?scope={}", urlencoding(s)),
            None => "/api/v1/variables".to_string(),
        };
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a new variable.
    ///
    /// `POST /api/v1/variables` with `{ name, value, scope? }`.
    pub async fn create_variable(
        &self,
        name: &str,
        value: &str,
        scope: Option<&str>,
    ) -> Result<StoredVariable> {
        let body = serde_json::json!({
            "name": name,
            "value": value,
            "scope": scope,
        })
        .to_string();
        let (status, resp) = self.post_json("/api/v1/variables", &body).await?;
        if !status.is_success() {
            Self::check_status(status, &resp)?;
        }
        Self::parse_json(&resp)
    }

    /// Fetch a single variable by id.
    ///
    /// `GET /api/v1/variables/{id}`.
    pub async fn get_variable(&self, id: &str) -> Result<StoredVariable> {
        let path = format!("/api/v1/variables/{}", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Update a variable's name and/or value.
    ///
    /// `PATCH /api/v1/variables/{id}` with `{ name?, value? }`.
    pub async fn update_variable(
        &self,
        id: &str,
        name: Option<&str>,
        value: Option<&str>,
    ) -> Result<StoredVariable> {
        let body = serde_json::json!({
            "name": name,
            "value": value,
        })
        .to_string();
        let path = format!("/api/v1/variables/{}", urlencoding(id));
        let (status, resp) = self.patch_json(&path, &body).await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    /// Delete a variable.
    ///
    /// `DELETE /api/v1/variables/{id}` -- 204 on success.
    pub async fn delete_variable(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/variables/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Projects
    // ------------------------------------------------------------------

    /// List all projects.
    ///
    /// `GET /api/v1/projects`
    pub async fn list_projects(&self) -> Result<Vec<serde_json::Value>> {
        let (status, body) = self.get("/api/v1/projects").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a new project.
    ///
    /// `POST /api/v1/projects`
    pub async fn create_project(&self, body: &serde_json::Value) -> Result<serde_json::Value> {
        let (status, resp) = self
            .post_json("/api/v1/projects", &body.to_string())
            .await?;
        if !status.is_success() {
            Self::check_status(status, &resp)?;
        }
        Self::parse_json(&resp)
    }

    /// Get a project by id.
    ///
    /// `GET /api/v1/projects/{id}`
    pub async fn get_project(&self, id: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/projects/{}", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Update a project (partial).
    ///
    /// `PATCH /api/v1/projects/{id}`
    pub async fn update_project(
        &self,
        id: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let path = format!("/api/v1/projects/{}", urlencoding(id));
        let (status, resp) = self.patch_json(&path, &body.to_string()).await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    /// Delete a project.
    ///
    /// `DELETE /api/v1/projects/{id}` -- 204 on success.
    pub async fn delete_project(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/projects/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// List deployments linked to a project.
    ///
    /// `GET /api/v1/projects/{id}/deployments`
    pub async fn list_project_deployments(&self, id: &str) -> Result<Vec<String>> {
        let path = format!("/api/v1/projects/{}/deployments", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Link a deployment to a project.
    ///
    /// `POST /api/v1/projects/{id}/deployments` with `{"deployment_name": "..."}`
    pub async fn link_project_deployment(&self, id: &str, deployment_name: &str) -> Result<()> {
        let path = format!("/api/v1/projects/{}/deployments", urlencoding(id));
        let payload = serde_json::json!({ "deployment_name": deployment_name });
        let (status, body) = self.post_json(&path, &payload.to_string()).await?;
        if !status.is_success() {
            Self::check_status(status, &body)?;
        }
        Ok(())
    }

    /// Unlink a deployment from a project.
    ///
    /// `DELETE /api/v1/projects/{id}/deployments/{name}`
    pub async fn unlink_project_deployment(&self, id: &str, deployment_name: &str) -> Result<()> {
        let path = format!(
            "/api/v1/projects/{}/deployments/{}",
            urlencoding(id),
            urlencoding(deployment_name),
        );
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// Clone or fast-forward pull a project's git repository on the daemon.
    ///
    /// `POST /api/v1/projects/{id}/pull` -- returns `{ project_id, git_url,
    /// branch, sha, path }`.
    pub async fn project_pull(&self, id: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/projects/{}/pull", urlencoding(id));
        // POST with an empty body; `post_json` requires a serialised payload
        // so we send `{}` here to match the route's lack of a request body.
        let (status, resp) = self.post_json(&path, "{}").await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    // ------------------------------------------------------------------
    // Webhooks
    // ------------------------------------------------------------------

    /// Get webhook configuration (URL + secret) for a project.
    ///
    /// `GET /api/v1/projects/{id}/webhook`
    pub async fn get_project_webhook(&self, id: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/projects/{}/webhook", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Rotate the webhook secret for a project.
    ///
    /// `POST /api/v1/projects/{id}/webhook/rotate`
    pub async fn rotate_project_webhook(&self, id: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/projects/{}/webhook/rotate", urlencoding(id));
        let (status, resp) = self.post_json(&path, "{}").await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    // ------------------------------------------------------------------
    // Credentials
    // ------------------------------------------------------------------

    /// List registry credentials.
    ///
    /// `GET /api/v1/credentials/registry`
    pub async fn list_registry_credentials(&self) -> Result<Vec<serde_json::Value>> {
        let (status, body) = self.get("/api/v1/credentials/registry").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a registry credential.
    ///
    /// `POST /api/v1/credentials/registry`
    pub async fn create_registry_credential(
        &self,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let (status, resp) = self
            .post_json("/api/v1/credentials/registry", &body.to_string())
            .await?;
        if !status.is_success() {
            Self::check_status(status, &resp)?;
        }
        Self::parse_json(&resp)
    }

    /// Delete a registry credential.
    ///
    /// `DELETE /api/v1/credentials/registry/{id}`
    pub async fn delete_registry_credential(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/credentials/registry/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// List git credentials.
    ///
    /// `GET /api/v1/credentials/git`
    pub async fn list_git_credentials(&self) -> Result<Vec<serde_json::Value>> {
        let (status, body) = self.get("/api/v1/credentials/git").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a git credential.
    ///
    /// `POST /api/v1/credentials/git`
    pub async fn create_git_credential(
        &self,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let (status, resp) = self
            .post_json("/api/v1/credentials/git", &body.to_string())
            .await?;
        if !status.is_success() {
            Self::check_status(status, &resp)?;
        }
        Self::parse_json(&resp)
    }

    /// Delete a git credential.
    ///
    /// `DELETE /api/v1/credentials/git/{id}`
    pub async fn delete_git_credential(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/credentials/git/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Env-scoped secrets
    // ------------------------------------------------------------------

    /// List secrets under a specific environment.
    ///
    /// `GET /api/v1/secrets?environment={env_id}`.
    pub async fn list_secrets_in_env(&self, env_id: &str) -> Result<Vec<SecretMetadataResponse>> {
        let path = format!("/api/v1/secrets?environment={}", urlencoding(env_id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Set (create or update) a secret under a specific environment.
    ///
    /// `POST /api/v1/secrets?environment={env_id}` with `{ name, value }`.
    pub async fn set_secret_in_env(
        &self,
        env_id: &str,
        name: &str,
        value: &str,
    ) -> Result<SecretMetadataResponse> {
        let path = format!("/api/v1/secrets?environment={}", urlencoding(env_id));
        let body = serde_json::json!({ "name": name, "value": value }).to_string();
        let (status, resp) = self.post_json(&path, &body).await?;
        if !status.is_success() {
            Self::check_status(status, &resp)?;
        }
        Self::parse_json(&resp)
    }

    /// Delete a secret under a specific environment.
    ///
    /// `DELETE /api/v1/secrets/{name}?environment={env_id}` — 204 on success.
    pub async fn delete_secret_in_env(&self, env_id: &str, name: &str) -> Result<()> {
        let path = format!(
            "/api/v1/secrets/{}?environment={}",
            urlencoding(name),
            urlencoding(env_id),
        );
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// Rotate a secret under a specific environment (admin only).
    ///
    /// `POST /api/v1/secrets/{name}/rotate?environment={env_id}` with
    /// `{ value }`. Returns the version before and after rotation.
    ///
    /// # Errors
    ///
    /// Returns an error if the daemon is unreachable, the caller lacks admin
    /// rights (`403`), the secret or environment is unknown (`404`), or the
    /// response body cannot be deserialized into [`RotateSecretResponse`].
    pub async fn rotate_secret_in_env(
        &self,
        env_id: &str,
        name: &str,
        new_value: &str,
    ) -> Result<RotateSecretResponse> {
        let path = format!(
            "/api/v1/secrets/{}/rotate?environment={}",
            urlencoding(name),
            urlencoding(env_id),
        );
        let body = serde_json::json!({ "value": new_value }).to_string();
        let (status, resp) = self.post_json(&path, &body).await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    /// Reveal a secret value under a specific environment (admin only).
    ///
    /// `GET /api/v1/secrets/{name}?environment={env_id}&reveal=true`.
    /// Returns the plaintext value extracted from the `SecretMetadataResponse`.
    pub async fn reveal_secret_in_env(&self, env_id: &str, name: &str) -> Result<String> {
        let path = format!(
            "/api/v1/secrets/{}?environment={}&reveal=true",
            urlencoding(name),
            urlencoding(env_id),
        );
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        let resp: SecretMetadataResponse = Self::parse_json(&body)?;
        resp.value
            .ok_or_else(|| anyhow::anyhow!("Daemon did not return a value for revealed secret"))
    }

    /// Reveal every secret in an env as plaintext (admin only). Single round-trip.
    ///
    /// `GET /api/v1/secrets/reveal-all?environment={env_id}`.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails, the caller lacks admin, or the env is unknown.
    pub async fn reveal_all_secrets_in_env(
        &self,
        env_id: &str,
    ) -> Result<std::collections::HashMap<String, String>> {
        let path = format!(
            "/api/v1/secrets/reveal-all?environment={}",
            urlencoding(env_id),
        );
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        let resp: RevealAllSecretsResponse = Self::parse_json(&body)?;
        Ok(resp.secrets)
    }

    /// Bulk-import secrets from a `.env`-style body into a specific environment.
    ///
    /// `POST /api/v1/secrets/bulk-import?environment={env_id}` with a
    /// `text/plain` body containing `KEY=VALUE` lines. Returns the raw import
    /// summary ({ created, updated, errors }) as a JSON value so the caller
    /// can render it verbatim.
    pub async fn bulk_import_secrets(
        &self,
        env_id: &str,
        dotenv_body: &str,
    ) -> Result<serde_json::Value> {
        let path = format!(
            "/api/v1/secrets/bulk-import?environment={}",
            urlencoding(env_id),
        );
        let (status, resp) = self.post_text(&path, dotenv_body).await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    // ------------------------------------------------------------------
    // Syncs
    // ------------------------------------------------------------------

    /// List all syncs.
    ///
    /// `GET /api/v1/syncs`
    pub async fn list_syncs(&self) -> Result<Vec<serde_json::Value>> {
        let (status, body) = self.get("/api/v1/syncs").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a new sync.
    ///
    /// `POST /api/v1/syncs`
    pub async fn create_sync(&self, body: &serde_json::Value) -> Result<serde_json::Value> {
        let (status, resp) = self.post_json("/api/v1/syncs", &body.to_string()).await?;
        if !status.is_success() {
            Self::check_status(status, &resp)?;
        }
        Self::parse_json(&resp)
    }

    /// Get the diff for a sync.
    ///
    /// `GET /api/v1/syncs/{id}/diff`
    pub async fn diff_sync(&self, id: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/syncs/{}/diff", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Apply a sync (dry-run in v1).
    ///
    /// `POST /api/v1/syncs/{id}/apply`
    pub async fn apply_sync(&self, id: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/syncs/{}/apply", urlencoding(id));
        let (status, body) = self.post_json(&path, "{}").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Delete a sync.
    ///
    /// `DELETE /api/v1/syncs/{id}` -- 204 on success.
    pub async fn delete_sync(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/syncs/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Tasks
    // ------------------------------------------------------------------

    /// List all tasks, optionally filtered by project id.
    ///
    /// `GET /api/v1/tasks[?project_id={id}]`
    pub async fn list_tasks(
        &self,
        project_id: Option<&str>,
    ) -> Result<Vec<zlayer_types::storage::StoredTask>> {
        let path = match project_id {
            Some(pid) => format!("/api/v1/tasks?project_id={}", urlencoding(pid)),
            None => "/api/v1/tasks".to_string(),
        };
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a new task.
    ///
    /// `POST /api/v1/tasks` with `{ name, kind, body, project_id? }`.
    pub async fn create_task(
        &self,
        name: &str,
        kind: &str,
        body_script: &str,
        project_id: Option<&str>,
    ) -> Result<zlayer_types::storage::StoredTask> {
        let payload = serde_json::json!({
            "name": name,
            "kind": kind,
            "body": body_script,
            "project_id": project_id,
        })
        .to_string();
        let (status, resp) = self.post_json("/api/v1/tasks", &payload).await?;
        if !status.is_success() {
            Self::check_status(status, &resp)?;
        }
        Self::parse_json(&resp)
    }

    /// Fetch a single task by id.
    ///
    /// `GET /api/v1/tasks/{id}`.
    pub async fn get_task(&self, id: &str) -> Result<zlayer_types::storage::StoredTask> {
        let path = format!("/api/v1/tasks/{}", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Execute a task synchronously.
    ///
    /// `POST /api/v1/tasks/{id}/run`.
    pub async fn run_task(&self, id: &str) -> Result<zlayer_types::storage::TaskRun> {
        let path = format!("/api/v1/tasks/{}/run", urlencoding(id));
        let (status, body) = self.post_json(&path, "{}").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// List past runs for a task.
    ///
    /// `GET /api/v1/tasks/{id}/runs`.
    pub async fn list_task_runs(&self, id: &str) -> Result<Vec<zlayer_types::storage::TaskRun>> {
        let path = format!("/api/v1/tasks/{}/runs", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Delete a task.
    ///
    /// `DELETE /api/v1/tasks/{id}` -- 204 on success.
    pub async fn delete_task(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/tasks/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Workflows
    // ------------------------------------------------------------------

    /// List all workflows.
    ///
    /// `GET /api/v1/workflows`
    pub async fn list_workflows(&self) -> Result<Vec<zlayer_types::storage::StoredWorkflow>> {
        let (status, body) = self.get("/api/v1/workflows").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a new workflow.
    ///
    /// `POST /api/v1/workflows` with `{ name, steps, project_id? }`.
    ///
    /// `steps_json` is a raw JSON array of workflow steps.
    pub async fn create_workflow(
        &self,
        name: &str,
        steps_json: &str,
        project_id: Option<&str>,
    ) -> Result<zlayer_types::storage::StoredWorkflow> {
        // Parse the steps JSON to validate it before sending
        let steps: serde_json::Value =
            serde_json::from_str(steps_json).context("Invalid JSON for --steps")?;

        let payload = serde_json::json!({
            "name": name,
            "steps": steps,
            "project_id": project_id,
        })
        .to_string();
        let (status, resp) = self.post_json("/api/v1/workflows", &payload).await?;
        if !status.is_success() {
            Self::check_status(status, &resp)?;
        }
        Self::parse_json(&resp)
    }

    /// Execute a workflow synchronously.
    ///
    /// `POST /api/v1/workflows/{id}/run`.
    pub async fn run_workflow(&self, id: &str) -> Result<zlayer_types::storage::WorkflowRun> {
        let path = format!("/api/v1/workflows/{}/run", urlencoding(id));
        let (status, body) = self.post_json(&path, "{}").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// List past runs for a workflow.
    ///
    /// `GET /api/v1/workflows/{id}/runs`.
    pub async fn list_workflow_runs(
        &self,
        id: &str,
    ) -> Result<Vec<zlayer_types::storage::WorkflowRun>> {
        let path = format!("/api/v1/workflows/{}/runs", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Delete a workflow.
    ///
    /// `DELETE /api/v1/workflows/{id}` -- 204 on success.
    pub async fn delete_workflow(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/workflows/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Notifiers
    // ------------------------------------------------------------------

    /// List all notifiers.
    ///
    /// `GET /api/v1/notifiers`
    pub async fn list_notifiers(&self) -> Result<Vec<zlayer_types::storage::StoredNotifier>> {
        let (status, body) = self.get("/api/v1/notifiers").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a new notifier.
    ///
    /// `POST /api/v1/notifiers` with `{ name, kind, config }`.
    pub async fn create_notifier(
        &self,
        name: &str,
        kind: zlayer_types::storage::NotifierKind,
        config: &zlayer_types::storage::NotifierConfig,
    ) -> Result<zlayer_types::storage::StoredNotifier> {
        let payload = serde_json::json!({
            "name": name,
            "kind": kind,
            "config": config,
        })
        .to_string();
        let (status, resp) = self.post_json("/api/v1/notifiers", &payload).await?;
        if !status.is_success() {
            Self::check_status(status, &resp)?;
        }
        Self::parse_json(&resp)
    }

    /// Send a test notification through a notifier.
    ///
    /// `POST /api/v1/notifiers/{id}/test`.
    pub async fn test_notifier(
        &self,
        id: &str,
    ) -> Result<zlayer_types::api::notifiers::TestNotifierResponse> {
        let path = format!("/api/v1/notifiers/{}/test", urlencoding(id));
        let (status, body) = self.post_json(&path, "{}").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Delete a notifier.
    ///
    /// `DELETE /api/v1/notifiers/{id}` -- 204 on success.
    pub async fn delete_notifier(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/notifiers/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Groups
    // ------------------------------------------------------------------

    /// List all groups.
    ///
    /// `GET /api/v1/groups`
    pub async fn list_groups(&self) -> Result<Vec<zlayer_types::storage::StoredUserGroup>> {
        let (status, body) = self.get("/api/v1/groups").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a group.
    ///
    /// `POST /api/v1/groups`
    pub async fn create_group(
        &self,
        req: &zlayer_types::api::groups::CreateGroupRequest,
    ) -> Result<zlayer_types::storage::StoredUserGroup> {
        let body = serde_json::to_string(req).context("Failed to serialise CreateGroupRequest")?;
        let (status, resp_body) = self.post_json("/api/v1/groups", &body).await?;
        Self::check_status(status, &resp_body)?;
        Self::parse_json(&resp_body)
    }

    /// Delete a group.
    ///
    /// `DELETE /api/v1/groups/{id}` -- 204 on success.
    pub async fn delete_group(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/groups/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// Add a member to a group.
    ///
    /// `POST /api/v1/groups/{id}/members`
    pub async fn add_group_member(&self, group_id: &str, user_id: &str) -> Result<()> {
        let req = zlayer_types::api::groups::AddMemberRequest {
            user_id: user_id.to_string(),
        };
        let body = serde_json::to_string(&req).context("Failed to serialise AddMemberRequest")?;
        let path = format!("/api/v1/groups/{}/members", urlencoding(group_id));
        let (status, resp_body) = self.post_json(&path, &body).await?;
        Self::check_status(status, &resp_body)?;
        Ok(())
    }

    /// Remove a member from a group.
    ///
    /// `DELETE /api/v1/groups/{id}/members/{user_id}`
    pub async fn remove_group_member(&self, group_id: &str, user_id: &str) -> Result<()> {
        let path = format!(
            "/api/v1/groups/{}/members/{}",
            urlencoding(group_id),
            urlencoding(user_id)
        );
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Permissions
    // ------------------------------------------------------------------

    /// List permissions for a subject.
    ///
    /// `GET /api/v1/permissions?user={id}` or `?group={id}`
    pub async fn list_permissions(
        &self,
        user: Option<&str>,
        group: Option<&str>,
    ) -> Result<Vec<zlayer_types::storage::StoredPermission>> {
        let mut path = "/api/v1/permissions".to_string();
        if let Some(uid) = user {
            path = format!("{path}?user={}", urlencoding(uid));
        } else if let Some(gid) = group {
            path = format!("{path}?group={}", urlencoding(gid));
        }
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Grant a permission.
    ///
    /// `POST /api/v1/permissions`
    pub async fn grant_permission(
        &self,
        req: &zlayer_types::api::permissions::GrantPermissionRequest,
    ) -> Result<zlayer_types::storage::StoredPermission> {
        let body =
            serde_json::to_string(req).context("Failed to serialise GrantPermissionRequest")?;
        let (status, resp_body) = self.post_json("/api/v1/permissions", &body).await?;
        Self::check_status(status, &resp_body)?;
        Self::parse_json(&resp_body)
    }

    /// Revoke a permission.
    ///
    /// `DELETE /api/v1/permissions/{id}` -- 204 on success.
    pub async fn revoke_permission(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/permissions/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// List permissions granted on a specific resource.
    ///
    /// `GET /api/v1/permissions/by-resource?kind={kind}&id={id}`.
    ///
    /// When `resource_id` is `Some`, returns exact-resource grants; when
    /// `None`, returns wildcard grants for the given `resource_kind`.
    pub async fn list_permissions_for_resource(
        &self,
        resource_kind: &str,
        resource_id: Option<&str>,
    ) -> Result<Vec<zlayer_types::storage::StoredPermission>> {
        use std::fmt::Write as _;
        let mut path = format!(
            "/api/v1/permissions/by-resource?kind={}",
            urlencoding(resource_kind)
        );
        if let Some(id) = resource_id {
            write!(path, "&id={}", urlencoding(id)).expect("writing to String is infallible");
        }
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    // ------------------------------------------------------------------
    // Audit
    // ------------------------------------------------------------------

    /// List audit entries with optional filters.
    ///
    /// `GET /api/v1/audit?user={id}&resource_kind={k}&limit={n}`
    pub async fn list_audit(
        &self,
        user: Option<&str>,
        resource_kind: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<zlayer_api::storage::AuditEntry>> {
        let mut params = Vec::new();
        if let Some(u) = user {
            params.push(format!("user={}", urlencoding(u)));
        }
        if let Some(rk) = resource_kind {
            params.push(format!("resource_kind={}", urlencoding(rk)));
        }
        if let Some(l) = limit {
            params.push(format!("limit={l}"));
        }
        let path = if params.is_empty() {
            "/api/v1/audit".to_string()
        } else {
            format!("/api/v1/audit?{}", params.join("&"))
        };
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    // ------------------------------------------------------------------
    // Image builds
    // ------------------------------------------------------------------

    /// Kick off an image build against a server-side context path.
    ///
    /// `POST /api/v1/build/json` with a JSON [`BuildSpec`]
    /// (`context_path`, `runtime`, `build_args`, `target`, `tags`,
    /// `no_cache`, `push`). The daemon returns `202 Accepted` with a
    /// [`BuildHandle`] carrying the `build_id` used to poll
    /// `GET /api/v1/build/{id}`, stream via
    /// `GET /api/v1/build/{id}/stream`, or collect logs from
    /// `GET /api/v1/build/{id}/logs`.
    ///
    /// The `context_path` must reference a directory that already exists on
    /// the daemon host; this method does not upload a tarball. For
    /// multipart-upload builds use the `/api/v1/build` endpoint directly.
    pub async fn start_build(&self, spec: BuildSpec) -> Result<BuildHandle> {
        let body = serde_json::to_string(&spec).context("Failed to serialize BuildSpec")?;
        let (status, resp) = self.post_json("/api/v1/build/json", &body).await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    /// Stream daemon lifecycle events from `GET /api/v1/events`.
    ///
    /// The endpoint returns NDJSON: one [`zlayer_api::DaemonEvent`] JSON
    /// object per line, terminated by `\n`. This method opens the stream,
    /// validates the response status, and returns an async stream that
    /// yields one parsed event per line.
    ///
    /// `follow` maps to the daemon's `?follow=<bool>` query parameter:
    /// `true` (default for the daemon) keeps the stream open until the
    /// connection drops or the daemon ends the response (e.g. terminal
    /// `close` line on subscriber lag); `false` makes the daemon emit an
    /// empty body and close immediately.
    ///
    /// Each `(k, v)` pair in `label_filters` is appended as a `label=k=v`
    /// query parameter (URL-encoded) and the daemon applies AND-semantics
    /// (an event passes only if every filter matches). Pass an empty slice
    /// for no filtering.
    ///
    /// # Stream semantics
    ///
    /// - Empty lines are skipped.
    /// - A line that fails to deserialize as `DaemonEvent` is logged at
    ///   `warn` and yielded as `Err`; the stream continues with the next
    ///   line rather than terminating on a single malformed entry.
    /// - An IO error on the underlying body terminates the stream after
    ///   yielding one `Err`.
    /// - On EOF the trailing partial line (no terminating newline) is
    ///   flushed and parsed before the stream ends.
    ///
    /// Caller is responsible for reconnecting if the stream ends and they
    /// want to resume.
    pub async fn events_stream(
        &self,
        follow: bool,
        label_filters: &[(String, String)],
    ) -> Result<
        std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<zlayer_api::DaemonEvent>> + Send>>,
    > {
        let path = build_events_path(follow, label_filters);
        let uri = self.uri(&path)?;

        let builder = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri(uri)
            .header("Host", "localhost")
            .header("Accept", "application/json");
        let builder = self.apply_session_auth(builder);
        let req = builder
            .body(Full::new(Bytes::new()))
            .context("Failed to build GET request for events stream")?;

        let resp = self
            .client
            .request(req)
            .await
            .with_context(|| format!("GET {path} (events) failed"))?;

        let (parts, body) = resp.into_parts();
        if !parts.status.is_success() {
            let collected = body
                .collect()
                .await
                .context("Failed to read events error body")?
                .to_bytes();
            Self::check_status(parts.status, &collected)?;
            unreachable!();
        }

        Ok(Box::pin(parse_ndjson_event_stream(body)))
    }

    // ------------------------------------------------------------------
    // Streaming endpoints (logs / stats / image pull)
    // ------------------------------------------------------------------

    /// Stream container logs from `GET /api/v1/containers/{id}/logs`.
    ///
    /// Returns the response body as an async stream of [`Bytes`] chunks
    /// exactly as the daemon emits them. The shape of each chunk depends on
    /// the `format_raw` flag:
    ///
    /// - `format_raw = true`: raw chunks of Docker's stdcopy framing
    ///   (`[stream, 0,0,0, BE_u32(len)] + payload`). Suitable for callers
    ///   that already speak the framed protocol (e.g. the Docker-compat
    ///   `/containers/{id}/logs` shim) and want byte-exact passthrough.
    /// - `format_raw = false`: NDJSON of [`LogChunk`] objects, one per
    ///   line. This method does **not** parse the lines — callers receive
    ///   raw chunked bytes and run their own NDJSON parser if they need
    ///   structured log records. Keeping the method format-agnostic lets
    ///   the same call site serve both the raw-binary and JSON cases
    ///   without two near-duplicate implementations.
    ///
    /// `follow=true` keeps the connection open until the daemon closes it;
    /// `follow=false` streams only the currently-buffered tail and ends.
    /// `tail` / `since` / `until` map directly to the matching query
    /// parameters on the daemon side. `timestamps`, `stdout`, `stderr`
    /// toggle the corresponding query flags.
    ///
    /// The argument list intentionally mirrors `LogsStreamOptions` 1:1
    /// rather than taking that struct, so callers can pass query
    /// parameters positionally without constructing an options builder.
    /// We accept the resulting many-args / many-bools clippy lints
    /// because a wrapper struct would not improve call-site readability
    /// for a method that already has a precedent (Docker SDK clients ship
    /// the same shape).
    #[allow(
        clippy::too_many_arguments,
        clippy::fn_params_excessive_bools,
        clippy::similar_names
    )]
    pub async fn stream_container_logs(
        &self,
        id: &str,
        follow: bool,
        tail: Option<u64>,
        since: Option<i64>,
        until: Option<i64>,
        timestamps: bool,
        stdout: bool,
        stderr: bool,
        format_raw: bool,
    ) -> Result<std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<Bytes>> + Send>>> {
        use std::fmt::Write as _;

        let mut path = format!(
            "/api/v1/containers/{}/logs?follow={}&timestamps={}&stdout={}&stderr={}",
            urlencoding(id),
            follow,
            timestamps,
            stdout,
            stderr,
        );
        if let Some(t) = tail {
            let _ = write!(path, "&tail={t}");
        }
        if let Some(s) = since {
            let _ = write!(path, "&since={s}");
        }
        if let Some(u) = until {
            let _ = write!(path, "&until={u}");
        }
        if format_raw {
            path.push_str("&format=raw");
        }

        let uri = self.uri(&path)?;

        let builder = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri(uri)
            .header("Host", "localhost");
        // `format_raw` requests an octet-stream body; otherwise the daemon
        // emits NDJSON. Set the matching `Accept` so middleware (compression,
        // logging) can branch correctly.
        let builder = if format_raw {
            builder.header("Accept", "application/octet-stream")
        } else {
            builder.header("Accept", "application/x-ndjson")
        };
        let builder = self.apply_session_auth(builder);
        let req = builder
            .body(Full::new(Bytes::new()))
            .context("Failed to build GET request for logs stream")?;

        let resp = self
            .client
            .request(req)
            .await
            .with_context(|| format!("GET {path} (logs) failed"))?;

        let (parts, body) = resp.into_parts();
        if !parts.status.is_success() {
            let collected = body
                .collect()
                .await
                .context("Failed to read logs error body")?
                .to_bytes();
            Self::check_status(parts.status, &collected)?;
            unreachable!();
        }

        Ok(Box::pin(raw_body_stream(body)))
    }

    /// Stream container stats samples from
    /// `GET /api/v1/containers/{id}/stats?stream={stream}`.
    ///
    /// When `stream = true` the daemon keeps the connection open and emits
    /// one NDJSON [`StatsSample`] per sampling tick (cadence is
    /// backend-defined; bollard's default is 1 Hz). When `stream = false`
    /// the daemon emits exactly one sample and closes.
    ///
    /// Each NDJSON line is deserialized into a [`StatsSample`]. Empty lines
    /// are skipped silently. A line that fails to deserialize is logged
    /// at `warn` and yielded as `Err`; the parser keeps reading subsequent
    /// lines so a single malformed sample doesn't end the subscription.
    /// IO errors on the underlying body terminate the stream after one
    /// `Err` item.
    pub async fn stream_container_stats(
        &self,
        id: &str,
        stream: bool,
    ) -> Result<std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<StatsSample>> + Send>>>
    {
        let path = format!(
            "/api/v1/containers/{}/stats?stream={}",
            urlencoding(id),
            stream,
        );
        let uri = self.uri(&path)?;

        let builder = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri(uri)
            .header("Host", "localhost")
            .header("Accept", "application/x-ndjson");
        let builder = self.apply_session_auth(builder);
        let req = builder
            .body(Full::new(Bytes::new()))
            .context("Failed to build GET request for stats stream")?;

        let resp = self
            .client
            .request(req)
            .await
            .with_context(|| format!("GET {path} (stats) failed"))?;

        let (parts, body) = resp.into_parts();
        if !parts.status.is_success() {
            let collected = body
                .collect()
                .await
                .context("Failed to read stats error body")?
                .to_bytes();
            Self::check_status(parts.status, &collected)?;
            unreachable!();
        }

        Ok(Box::pin(parse_ndjson_typed_stream::<_, StatsSample>(
            body, "stats",
        )))
    }

    /// Stream image-pull progress from
    /// `POST /api/v1/images/pull?stream=true`.
    ///
    /// Body: `{"reference": "<image>", "registry_auth": <auth or null>}`,
    /// matching the field names used by the existing blocking
    /// `POST /api/v1/images/pull` endpoint (see
    /// [`zlayer_types::api::images::PullImageRequest`]). The daemon
    /// upgrades to NDJSON when `?stream=true` is set on the query string,
    /// emitting one [`PullProgress`] per layer/status tick followed by
    /// exactly one terminal `Done` event on success.
    ///
    /// As with [`Self::stream_container_stats`], NDJSON parse failures
    /// surface as `Err` items but do not end the stream — only an
    /// underlying IO error terminates iteration.
    pub async fn stream_image_pull(
        &self,
        image: &str,
        auth: Option<zlayer_types::spec::RegistryAuth>,
    ) -> Result<std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<PullProgress>> + Send>>>
    {
        let body_json = serde_json::to_string(&serde_json::json!({
            "reference": image,
            "registry_auth": auth,
        }))
        .context("Failed to serialize image pull request body")?;

        let path = "/api/v1/images/pull?stream=true";
        let uri = self.uri(path)?;

        let builder = hyper::Request::builder()
            .method(hyper::Method::POST)
            .uri(uri)
            .header("Host", "localhost")
            .header("Content-Type", "application/json")
            .header("Accept", "application/x-ndjson");
        let builder = self.apply_session_auth(builder);
        let req = builder
            .body(Full::new(Bytes::from(body_json)))
            .context("Failed to build POST request for image pull stream")?;

        let resp = self
            .client
            .request(req)
            .await
            .with_context(|| format!("POST {path} (pull stream) failed"))?;

        let (parts, body) = resp.into_parts();
        if !parts.status.is_success() {
            let collected = body
                .collect()
                .await
                .context("Failed to read pull error body")?
                .to_bytes();
            Self::check_status(parts.status, &collected)?;
            unreachable!();
        }

        Ok(Box::pin(parse_ndjson_typed_stream::<_, PullProgress>(
            body, "pull",
        )))
    }

    // ------------------------------------------------------------------
    // Exec instances (Docker-shaped create/start/inspect/resize flow)
    // ------------------------------------------------------------------

    /// Create a new exec instance against the named container.
    ///
    /// `POST /api/v1/containers/{id}/exec` with a JSON [`ExecOptions`]
    /// body. The daemon allocates a 64-char lowercase hex exec ID and
    /// returns it inside a [`CreateExecResponse`]. The exec is *not*
    /// started by this call — pass the returned ID to
    /// [`DaemonClient::start_exec_pty`] (interactive) or other start
    /// endpoints to begin execution.
    pub async fn create_exec(&self, container_id: &str, opts: ExecOptions) -> Result<String> {
        let path = format!("/api/v1/containers/{}/exec", urlencoding(container_id));
        let body = serde_json::to_string(&opts).context("Failed to serialize ExecOptions")?;
        let (status, resp) = self.post_json(&path, &body).await?;
        Self::check_status(status, &resp)?;
        let parsed: CreateExecResponse = Self::parse_json(&resp)?;
        Ok(parsed.id)
    }

    /// Inspect a previously-created exec instance.
    ///
    /// `GET /api/v1/exec/{id}/json` returning the full [`ExecInstanceJson`]
    /// record (id, container ref, planned options, lifecycle timestamps,
    /// exit code).
    pub async fn inspect_exec(&self, exec_id: &str) -> Result<ExecInstanceJson> {
        let path = format!("/api/v1/exec/{}/json", urlencoding(exec_id));
        let (status, resp) = self.get(&path).await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    /// Resize the PTY allocated to an exec instance.
    ///
    /// `POST /api/v1/exec/{id}/resize` with a JSON `{rows, cols}` body.
    /// No-op when the exec was created with `tty=false`; the daemon still
    /// returns 204 in that case so callers don't have to special-case it.
    pub async fn resize_exec(&self, exec_id: &str, rows: u16, cols: u16) -> Result<()> {
        let path = format!("/api/v1/exec/{}/resize", urlencoding(exec_id));
        let body = serde_json::json!({ "rows": rows, "cols": cols }).to_string();
        let (status, resp) = self.post_json(&path, &body).await?;
        Self::check_status(status, &resp)?;
        Ok(())
    }

    /// Resize the PTY allocated to a running container.
    ///
    /// `POST /api/v1/containers/{id}/resize` with a JSON `{rows, cols}`
    /// body. Equivalent to the Docker `POST /containers/{id}/resize`
    /// endpoint and used by interactive `attach` sessions.
    pub async fn resize_container(&self, container_id: &str, rows: u16, cols: u16) -> Result<()> {
        let path = format!("/api/v1/containers/{}/resize", urlencoding(container_id));
        let body = serde_json::json!({ "rows": rows, "cols": cols }).to_string();
        let (status, resp) = self.post_json(&path, &body).await?;
        Self::check_status(status, &resp)?;
        Ok(())
    }

    /// Start an exec instance interactively, returning a duplex
    /// WebSocket-backed [`ExecPtyConnection`].
    ///
    /// Opens a WebSocket to `POST /api/v1/exec/{id}/start` over the same
    /// transport [`DaemonClient`] uses for HTTP (Unix-domain socket on Unix,
    /// TCP loopback on Windows). The connection follows this protocol:
    ///
    /// - **Binary frames**: shuttle stdin (client -> daemon) and stdout /
    ///   stderr (daemon -> client). When `tty=false` the daemon emits the
    ///   stdcopy framing Docker uses; when `tty=true` it emits raw PTY
    ///   bytes. Either way `ExecPtyConnection::reader` surfaces each
    ///   binary frame as a single [`Bytes`] item.
    /// - **JSON text frames**: `{"resize":{"rows":R,"cols":C}}` carries
    ///   PTY-resize hints from client to daemon. Generated automatically
    ///   from `(rows, cols)` items sent on the returned `resize` channel.
    /// - **Close frame**: code 1000 with the exit code formatted as
    ///   decimal in the close-frame `reason` text. The exit future
    ///   resolves once such a close frame arrives (or with `Ok(None)` on
    ///   a transport-level close that didn't include an exit code).
    pub async fn start_exec_pty(&self, exec_id: &str, tty: bool) -> Result<ExecPtyConnection> {
        let path = format!("/api/v1/exec/{}/start?tty={}", urlencoding(exec_id), tty);
        let stream = self.connect_websocket_stream(&path).await?;

        // 64-message bound on every channel: high enough that interactive
        // typing and SIGWINCH bursts don't block, low enough that a stuck
        // peer can't make us buffer unbounded data.
        let (out_tx, out_rx) = tokio::sync::mpsc::channel::<ExecPtyOutbound>(64);
        let (resize_tx, mut resize_rx) = tokio::sync::mpsc::channel::<(u16, u16)>(64);
        let (read_tx, read_rx) = tokio::sync::mpsc::channel::<Result<Bytes>>(64);
        let (exit_tx, exit_rx) = tokio::sync::oneshot::channel::<Result<Option<i32>>>();

        // Forward `(rows, cols)` items from the public resize channel onto
        // the unified outbound channel as `Resize` commands. Lives on its
        // own task so the session loop only has to drain a single channel.
        let resize_forward_tx = out_tx.clone();
        tokio::spawn(async move {
            while let Some((rows, cols)) = resize_rx.recv().await {
                if resize_forward_tx
                    .send(ExecPtyOutbound::Resize { rows, cols })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        tokio::spawn(run_exec_pty_session(stream, out_rx, read_tx, exit_tx));

        let reader = Box::pin(tokio_stream::wrappers::ReceiverStream::new(read_rx));
        let writer = ExecPtyWriter { tx: out_tx };
        let exit = Box::pin(async move {
            match exit_rx.await {
                Ok(res) => res,
                Err(_) => Ok(None),
            }
        });

        Ok(ExecPtyConnection {
            reader,
            writer,
            resize: resize_tx,
            exit,
        })
    }

    /// Open a raw WebSocket to the daemon at `path`.
    ///
    /// Bridges the platform-specific transport (`UnixStream` on Unix,
    /// `TcpStream` on Windows) into `tokio_tungstenite::client_async`.
    /// The session-level auth header is attached when a session token is
    /// available; on Unix the daemon's peer-credential middleware handles
    /// local-admin auth out of band, matching the regular HTTP path.
    async fn connect_websocket_stream(
        &self,
        path: &str,
    ) -> Result<tokio_tungstenite::WebSocketStream<DaemonStreamForExec>> {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        use tokio_tungstenite::tungstenite::handshake::client::generate_key;

        let stream = self.dial_raw_stream().await?;

        // Build a minimal WebSocket handshake request. We have to use
        // `IntoClientRequest` so tokio-tungstenite recognises the URL
        // shape, then layer the upgrade headers on top because
        // `client_async` does not generate them automatically.
        #[cfg(unix)]
        let url = format!("ws://localhost{path}");
        #[cfg(windows)]
        let url = format!("ws://{}{path}", self.endpoint);

        let mut request = url
            .as_str()
            .into_client_request()
            .with_context(|| format!("invalid websocket URL {url:?}"))?;
        let headers = request.headers_mut();
        headers.insert(
            hyper::header::CONNECTION,
            hyper::header::HeaderValue::from_static("Upgrade"),
        );
        headers.insert(
            hyper::header::UPGRADE,
            hyper::header::HeaderValue::from_static("websocket"),
        );
        headers.insert(
            hyper::header::SEC_WEBSOCKET_VERSION,
            hyper::header::HeaderValue::from_static("13"),
        );
        let key = generate_key();
        headers.insert(
            hyper::header::SEC_WEBSOCKET_KEY,
            hyper::header::HeaderValue::from_str(&key)
                .context("websocket key was not valid header value")?,
        );

        if let Some(auth) = self.bearer_auth_header() {
            headers.insert(
                hyper::header::AUTHORIZATION,
                hyper::header::HeaderValue::from_str(&auth)
                    .context("Authorization header value was not valid")?,
            );
        }

        let (ws, _resp) = tokio_tungstenite::client_async(request, stream)
            .await
            .with_context(|| format!("websocket handshake at {path} failed"))?;

        Ok(ws)
    }

    /// Build a `Bearer <token>` header value to attach to the WebSocket
    /// upgrade request, when a session token (or, on Windows, a persisted
    /// admin bearer) is available. Returns `None` when no auth applies.
    #[cfg_attr(unix, allow(clippy::unused_self))]
    fn bearer_auth_header(&self) -> Option<String> {
        if let Ok(Some(session)) = crate::session::read_session() {
            if !session.is_expired() {
                return Some(format!("Bearer {}", session.token));
            }
        }
        #[cfg(windows)]
        {
            if let Some(bearer) = self.bearer.as_deref() {
                return Some(format!("Bearer {bearer}"));
            }
        }
        None
    }

    /// Open a raw transport stream to the daemon. Used as the foundation
    /// for the WebSocket upgrade; the regular HTTP methods reuse the
    /// hyper client and never hit this path.
    #[cfg(unix)]
    async fn dial_raw_stream(&self) -> Result<DaemonStreamForExec> {
        let stream = tokio::net::UnixStream::connect(&self.socket_path)
            .await
            .with_context(|| {
                format!(
                    "Failed to dial daemon socket at {} for websocket",
                    self.socket_path.display()
                )
            })?;
        Ok(stream)
    }

    #[cfg(windows)]
    async fn dial_raw_stream(&self) -> Result<DaemonStreamForExec> {
        let stream = tokio::net::TcpStream::connect(self.endpoint)
            .await
            .with_context(|| {
                format!("Failed to dial daemon TCP {} for websocket", self.endpoint)
            })?;
        Ok(stream)
    }

    // ------------------------------------------------------------------
    // Cluster / nodes (typed)
    //
    // Thin wrappers over the daemon's `/api/v1/cluster/*` and
    // `/api/v1/nodes/*` endpoints. Used by the upcoming Docker-compat Swarm
    // bridge; SDK consumers can also reach these directly without
    // hand-rolling JSON.
    // ------------------------------------------------------------------

    /// List every cluster node visible in Raft state.
    ///
    /// `GET /api/v1/cluster/nodes` -- response shape is
    /// [`zlayer_api::ClusterNodeSummary`] (the canonical type lives in the
    /// API crate; the client crate already depends on `zlayer-api`, so no
    /// re-export was necessary).
    pub async fn cluster_nodes(&self) -> Result<Vec<zlayer_api::ClusterNodeSummary>> {
        let (status, body) = self.get("/api/v1/cluster/nodes").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Inspect a single node by id.
    ///
    /// `GET /api/v1/nodes/{id}`. The daemon serves the richer
    /// [`zlayer_types::api::nodes::NodeDetails`] DTO (resources + service
    /// list), not just a [`zlayer_api::ClusterNodeSummary`], so this method
    /// returns the richer document. The Swarm bridge maps it to
    /// `Node{ID,Description,Status,...}` on the wire.
    pub async fn node_inspect(&self, id: &str) -> Result<zlayer_types::api::nodes::NodeDetails> {
        let path = format!("/api/v1/nodes/{}", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Replace the labels on a node.
    ///
    /// `POST /api/v1/nodes/{id}/labels` with a JSON body matching the
    /// daemon's [`zlayer_types::api::nodes::UpdateLabelsRequest`] (a
    /// `labels` map plus a `remove` list of keys to drop). This helper
    /// always sends an empty `remove` list because the Docker-compat
    /// caller's spec only supplies the desired final label set; if you
    /// need to drop specific keys, drop down to `post_json` directly.
    pub async fn node_set_labels(
        &self,
        id: &str,
        labels: std::collections::HashMap<String, String>,
    ) -> Result<()> {
        // The daemon-side `UpdateLabelsRequest` (mirrored in
        // `zlayer_types::api::nodes`) only derives `Deserialize`, so we
        // build the JSON body inline rather than re-derive `Serialize` on
        // a public DTO whose stability promise is one-way.
        let path = format!("/api/v1/nodes/{}/labels", urlencoding(id));
        let empty_remove: [&str; 0] = [];
        let payload = serde_json::json!({
            "labels": labels,
            "remove": empty_remove,
        });
        let (status, resp) = self.post_json(&path, &payload.to_string()).await?;
        Self::check_status(status, &resp)?;
        Ok(())
    }

    /// Forward a cluster-join request to the local daemon.
    ///
    /// `POST /api/v1/cluster/join`. The body is constructed by the caller
    /// (typically the Docker-compat `/swarm/join` shim) so this helper does
    /// not enforce any one DTO -- the daemon's
    /// [`zlayer_api::ClusterJoinRequest`] only derives `Deserialize`, so a
    /// `serde_json::Value` is the cheapest interchange format that does not
    /// drag a new `Serialize` impl across the API boundary.
    ///
    /// Returns the raw response body as JSON on 2xx; on non-success the
    /// daemon's error envelope is propagated via [`Self::check_status`].
    pub async fn cluster_join(&self, body: &serde_json::Value) -> Result<serde_json::Value> {
        let (status, resp) = self
            .post_json("/api/v1/cluster/join", &body.to_string())
            .await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    /// Trigger a cluster signing-key rotation.
    ///
    /// `POST /api/v1/cluster/rotate-signing-key`. The daemon owns the
    /// leader-vs-worker decision: when this method is invoked on a worker
    /// node the daemon forwards the request to the current Raft leader; on
    /// the leader it rotates the on-disk keystore in place. The previously
    /// active key is moved into a grace window (caller-controlled via
    /// [`RotateSigningKeyRequest::grace`], default `7d`) where it continues
    /// to verify in-flight join tokens until expiry.
    ///
    /// Wave 5B.4 added the server-side handler; Wave 5B.3 added this client
    /// wrapper and the matching `zlayer node|cluster rotate-signing-key`
    /// CLI subcommands so admins can drive the rotation without hand-rolling
    /// curl invocations.
    pub async fn cluster_rotate_signing_key(
        &self,
        req: &zlayer_types::api::cluster::RotateSigningKeyRequest,
    ) -> Result<zlayer_types::api::cluster::RotateSigningKeyResponse> {
        let body =
            serde_json::to_string(req).context("Failed to serialize RotateSigningKeyRequest")?;
        let (status, resp) = self
            .post_json("/api/v1/cluster/rotate-signing-key", &body)
            .await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    /// Revoke a previously-issued cluster join token.
    ///
    /// `POST /api/v1/cluster/revoke-token` (admin auth required). The
    /// server hashes the supplied `token_or_hash` to its canonical form
    /// before proposing a `SecretsRaftOp::RevokeToken`, so every node
    /// converges on the same revocation set within one Raft commit.
    ///
    /// Wave 7.4 added the server-side handler; Wave 7.6 added this
    /// client wrapper and the matching `zlayer cluster revoke-token`
    /// CLI subcommand.
    pub async fn cluster_revoke_token(
        &self,
        req: &zlayer_types::api::cluster::RevokeTokenRequest,
    ) -> Result<zlayer_types::api::cluster::RevokeTokenResponse> {
        let body = serde_json::to_string(req).context("Failed to serialize RevokeTokenRequest")?;
        let (status, resp) = self
            .post_json("/api/v1/cluster/revoke-token", &body)
            .await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    /// List currently-active token revocations.
    ///
    /// `GET /api/v1/cluster/revocations` (admin auth required). Returns
    /// a point-in-time view of the local Raft state machine's
    /// un-expired revocations; entries auto-prune at apply time.
    pub async fn cluster_list_revocations(
        &self,
    ) -> Result<zlayer_types::api::cluster::RevocationListResponse> {
        let (status, resp) = self.get("/api/v1/cluster/revocations").await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    // ------------------------------------------------------------------
    // Overlay (typed)
    // ------------------------------------------------------------------

    /// Typed overlay status.
    ///
    /// `GET /api/v1/overlay/status`. Sibling of the legacy
    /// [`Self::get_overlay_status`] which returns a raw
    /// `serde_json::Value`; this one deserializes into the strongly-typed
    /// [`zlayer_types::api::overlay::OverlayStatusResponse`] used by the
    /// Docker-compat shim.
    pub async fn overlay_status(
        &self,
    ) -> Result<zlayer_types::api::overlay::OverlayStatusResponse> {
        let (status, body) = self.get("/api/v1/overlay/status").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    // ------------------------------------------------------------------
    // Secrets (typed)
    //
    // Sibling helpers to [`Self::list_secrets`] / [`Self::create_secret`] /
    // [`Self::delete_secret`] which all return `serde_json::Value`. These
    // versions deserialize into
    // [`zlayer_types::api::secrets::SecretMetadataResponse`] (and friends)
    // and accept an optional scope query parameter.
    // ------------------------------------------------------------------

    /// List secret metadata in a scope.
    ///
    /// `GET /api/v1/secrets[?scope=<scope>]`. When `scope` is `None` the
    /// daemon falls back to the literal `"default"` scope (legacy path);
    /// pass `Some("...")` for project / env-style namespacing.
    pub async fn secrets_list(
        &self,
        scope: Option<&str>,
    ) -> Result<Vec<zlayer_types::api::secrets::SecretMetadataResponse>> {
        let path = match scope {
            Some(s) => format!("/api/v1/secrets?scope={}", urlencoding(s)),
            None => "/api/v1/secrets".to_string(),
        };
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create or update a secret.
    ///
    /// `POST /api/v1/secrets[?scope=<scope>]` with a JSON body matching
    /// [`zlayer_types::api::secrets::CreateSecretRequest`]. The optional
    /// `scope` is sent as a query parameter (the body's `scope` field is
    /// left unset, since the daemon rejects requests that mix the body and
    /// query forms).
    pub async fn secrets_create(
        &self,
        name: &str,
        value: &str,
        scope: Option<&str>,
    ) -> Result<zlayer_types::api::secrets::SecretMetadataResponse> {
        let path = match scope {
            Some(s) => format!("/api/v1/secrets?scope={}", urlencoding(s)),
            None => "/api/v1/secrets".to_string(),
        };
        let payload = serde_json::json!({ "name": name, "value": value });
        let (status, body) = self.post_json(&path, &payload.to_string()).await?;
        if !status.is_success() {
            Self::check_status(status, &body)?;
        }
        Self::parse_json(&body)
    }

    /// Delete a secret by name.
    ///
    /// `DELETE /api/v1/secrets/{name}[?scope=<scope>]` -- returns 204 on
    /// success.
    pub async fn secrets_delete(&self, name: &str, scope: Option<&str>) -> Result<()> {
        let path = match scope {
            Some(s) => format!(
                "/api/v1/secrets/{}?scope={}",
                urlencoding(name),
                urlencoding(s),
            ),
            None => format!("/api/v1/secrets/{}", urlencoding(name)),
        };
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// Rotate a secret -- overwrite with a new value, returning the
    /// previous and new versions.
    ///
    /// `POST /api/v1/secrets/{name}/rotate[?scope=<scope>]` with a JSON
    /// body matching [`zlayer_types::api::secrets::RotateSecretRequest`].
    /// The handler returns
    /// [`zlayer_types::api::secrets::RotateSecretResponse`] (richer than
    /// the spec's `SecretMeta` -- it carries `previous_version` and
    /// `new_version` rather than the full metadata document).
    pub async fn secrets_rotate(
        &self,
        name: &str,
        value: &str,
        scope: Option<&str>,
    ) -> Result<zlayer_types::api::secrets::RotateSecretResponse> {
        let path = match scope {
            Some(s) => format!(
                "/api/v1/secrets/{}/rotate?scope={}",
                urlencoding(name),
                urlencoding(s),
            ),
            None => format!("/api/v1/secrets/{}/rotate", urlencoding(name)),
        };
        let payload = serde_json::json!({ "value": value });
        let (status, body) = self.post_json(&path, &payload.to_string()).await?;
        if !status.is_success() {
            Self::check_status(status, &body)?;
        }
        Self::parse_json(&body)
    }

    // ------------------------------------------------------------------
    // Variables (typed)
    // ------------------------------------------------------------------

    /// List every global variable.
    ///
    /// `GET /api/v1/variables`. The daemon's contract: with no `scope`
    /// query param it returns variables whose `scope IS NULL` (globals
    /// only). To list project-scoped variables, drop down to `get` with
    /// `?scope={project_id}` directly.
    pub async fn variables_list(&self) -> Result<Vec<StoredVariable>> {
        let (status, body) = self.get("/api/v1/variables").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a new variable.
    ///
    /// `POST /api/v1/variables` with a JSON body matching
    /// [`zlayer_types::api::variables::CreateVariableRequest`]. The
    /// optional `env_id` is forwarded as the body's `scope` field --
    /// matching the daemon's project / env scope convention.
    pub async fn variables_create(
        &self,
        name: &str,
        value: &str,
        env_id: Option<&str>,
    ) -> Result<StoredVariable> {
        let payload = serde_json::json!({
            "name": name,
            "value": value,
            "scope": env_id,
        });
        let (status, body) = self
            .post_json("/api/v1/variables", &payload.to_string())
            .await?;
        if !status.is_success() {
            Self::check_status(status, &body)?;
        }
        Self::parse_json(&body)
    }

    /// Update a variable's value.
    ///
    /// `PATCH /api/v1/variables/{id}` with a JSON body matching
    /// [`zlayer_types::api::variables::UpdateVariableRequest`]. Only the
    /// `value` field is sent; rename-by-PATCH is intentionally not exposed
    /// here.
    pub async fn variables_patch(&self, id: &str, value: &str) -> Result<StoredVariable> {
        let path = format!("/api/v1/variables/{}", urlencoding(id));
        let payload = serde_json::json!({ "value": value });
        let (status, body) = self.patch_json(&path, &payload.to_string()).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Delete a variable by id.
    ///
    /// `DELETE /api/v1/variables/{id}` -- returns 204 on success.
    pub async fn variables_delete(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/variables/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Service replicas (typed) -- used by the Docker-compat `/tasks` shim.
    // ------------------------------------------------------------------

    /// List the running container replicas of a service inside a deployment.
    ///
    /// `GET /api/v1/deployments/{deployment}/services/{service}/containers`.
    /// Sibling of [`Self::list_containers`] (which returns
    /// `Vec<serde_json::Value>`); this version deserializes into the typed
    /// [`zlayer_types::api::services::ContainerSummary`] DTO.
    pub async fn deployment_replicas(
        &self,
        deployment: &str,
        service: &str,
    ) -> Result<Vec<zlayer_types::api::services::ContainerSummary>> {
        let path = format!(
            "/api/v1/deployments/{}/services/{}/containers",
            urlencoding(deployment),
            urlencoding(service),
        );
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }
}

/// Concrete platform-specific stream type used as the WebSocket transport
/// for [`DaemonClient::start_exec_pty`]. On Unix this is a Unix-domain
/// socket; on Windows it is a TCP loopback stream.
#[cfg(unix)]
type DaemonStreamForExec = tokio::net::UnixStream;
#[cfg(windows)]
type DaemonStreamForExec = tokio::net::TcpStream;

/// Drive the WebSocket session for one [`DaemonClient::start_exec_pty`]
/// connection.
///
/// Runs on its own tokio task. Three concurrent jobs:
///
/// - Drain `out_rx` and forward each [`ExecPtyOutbound`] command as the
///   matching WebSocket frame (binary / text / close).
/// - Read incoming frames and forward binary payloads on `read_tx`. Pings
///   are auto-replied with pongs by `tokio_tungstenite` and never surface.
///   Text frames are ignored (the daemon only sends them for resize echo
///   in the future).
/// - On a close frame, parse the exit code from the `reason` field and
///   resolve `exit_tx`. Any unparsable reason resolves to `Ok(None)` so
///   callers can still detect "session ended" without crashing.
async fn run_exec_pty_session<S>(
    stream: tokio_tungstenite::WebSocketStream<S>,
    mut out_rx: tokio::sync::mpsc::Receiver<ExecPtyOutbound>,
    read_tx: tokio::sync::mpsc::Sender<Result<Bytes>>,
    exit_tx: tokio::sync::oneshot::Sender<Result<Option<i32>>>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    use futures_util::{SinkExt as _, StreamExt as _};
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    let (mut sink, mut source) = stream.split();
    let mut exit_tx = Some(exit_tx);
    let mut exit_code_seen: Option<i32> = None;

    loop {
        tokio::select! {
            // Outbound: pump caller commands onto the wire. `biased`-free
            // select is fine: starvation isn't a concern because the
            // inbound side is bounded by the daemon's send cadence.
            cmd = out_rx.recv() => {
                let Some(cmd) = cmd else { break; };
                match cmd {
                    ExecPtyOutbound::Stdin(data) => {
                        if let Err(e) = sink.send(WsMessage::Binary(data)).await {
                            let _ = read_tx.send(Err(anyhow::anyhow!(
                                "exec PTY stdin send failed: {e}"
                            ))).await;
                            break;
                        }
                    }
                    ExecPtyOutbound::Resize { rows, cols } => {
                        let payload = serde_json::json!({
                            "resize": { "rows": rows, "cols": cols }
                        })
                        .to_string();
                        if let Err(e) = sink.send(WsMessage::text(payload)).await {
                            let _ = read_tx.send(Err(anyhow::anyhow!(
                                "exec PTY resize send failed: {e}"
                            ))).await;
                            break;
                        }
                    }
                    ExecPtyOutbound::Close => {
                        let _ = sink.send(WsMessage::Close(None)).await;
                        break;
                    }
                }
            }

            // Inbound: forward stdout/stderr to the caller, watch for the
            // close frame so we can resolve `exit_tx`.
            msg = source.next() => {
                match msg {
                    Some(Ok(WsMessage::Binary(data))) => {
                        // `WsMessage::Binary` is already `bytes::Bytes`;
                        // forward the buffer verbatim.
                        if read_tx.send(Ok(data)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(WsMessage::Close(frame))) => {
                        // Parse the exit code out of the close-frame reason.
                        // Empty / non-numeric reasons resolve to `None` so
                        // the caller still detects "session ended".
                        if let Some(frame) = frame {
                            let reason: &str = frame.reason.as_ref();
                            exit_code_seen = reason.trim().parse::<i32>().ok();
                        }
                        // Send a courtesy close back; ignore failures.
                        let _ = sink.send(WsMessage::Close(None)).await;
                        break;
                    }
                    Some(Ok(WsMessage::Text(_) | _)) => {
                        // Text frames are reserved for control messages
                        // (currently only the client -> daemon resize hint),
                        // and Ping / Pong / Frame are handled internally by
                        // tokio-tungstenite. Drop both silently.
                    }
                    Some(Err(e)) => {
                        let _ = read_tx.send(Err(anyhow::anyhow!(
                            "exec PTY websocket read failed: {e}"
                        ))).await;
                        break;
                    }
                    None => break,
                }
            }
        }
    }

    if let Some(tx) = exit_tx.take() {
        let _ = tx.send(Ok(exit_code_seen));
    }
}

// ---------------------------------------------------------------------------
// NDJSON event-stream helpers
// ---------------------------------------------------------------------------

/// Build the `/api/v1/events` path with `follow=<bool>` plus zero or more
/// URL-encoded `label=k=v` filters appended as repeated query parameters.
fn build_events_path(follow: bool, label_filters: &[(String, String)]) -> String {
    use std::fmt::Write as _;

    let mut path = format!("/api/v1/events?follow={follow}");
    for (k, v) in label_filters {
        // The handler splits on the first `=`, so we URL-encode the key
        // and the value separately and join them with a literal `=`.
        let _ = write!(&mut path, "&label={}={}", urlencoding(k), urlencoding(v));
    }
    path
}

/// Convert a hyper response body into an async stream of parsed
/// [`zlayer_api::DaemonEvent`] items, one per NDJSON line.
///
/// Behaviour:
///   * Empty lines are skipped.
///   * A line that fails JSON-parsing is logged at `warn` and yielded as
///     `Err`; the stream continues with the next line.
///   * An IO error on the body is yielded as `Err` and terminates the
///     stream.
///   * On EOF, any trailing bytes without a terminating newline are
///     parsed as one final line.
///
/// Extracted from [`DaemonClient::events_stream`] so the parse loop is
/// testable without a live daemon.
fn parse_ndjson_event_stream<B>(
    body: B,
) -> impl futures_util::Stream<Item = Result<zlayer_api::DaemonEvent>> + Send
where
    B: hyper::body::Body<Data = Bytes> + Send + Unpin + 'static,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    use futures_util::stream::{self, StreamExt as _};
    use http_body_util::BodyStream;

    stream::unfold(
        (BodyStream::new(body), Vec::<u8>::new(), false),
        |(mut body_stream, mut buf, mut done)| async move {
            loop {
                // Drain any complete lines already in the buffer.
                if let Some(idx) = buf.iter().position(|b| *b == b'\n') {
                    let mut line: Vec<u8> = buf.drain(..=idx).collect();
                    // Strip the trailing `\n` (and tolerate `\r\n`).
                    line.pop();
                    if line.last() == Some(&b'\r') {
                        line.pop();
                    }
                    if line.is_empty() {
                        continue;
                    }
                    let parsed = serde_json::from_slice::<zlayer_api::DaemonEvent>(&line)
                        .with_context(|| {
                            format!(
                                "failed to parse NDJSON event line: {}",
                                String::from_utf8_lossy(&line)
                            )
                        });
                    if let Err(ref e) = parsed {
                        tracing::warn!(
                            error = %e,
                            "skipping malformed NDJSON event line"
                        );
                    }
                    return Some((parsed, (body_stream, buf, done)));
                }

                if done {
                    // Flush any trailing partial line on EOF before ending.
                    if !buf.is_empty() {
                        let line: Vec<u8> = std::mem::take(&mut buf);
                        let parsed = serde_json::from_slice::<zlayer_api::DaemonEvent>(&line)
                            .with_context(|| {
                                format!(
                                    "failed to parse trailing NDJSON event line: {}",
                                    String::from_utf8_lossy(&line)
                                )
                            });
                        if let Err(ref e) = parsed {
                            tracing::warn!(
                                error = %e,
                                "skipping malformed trailing NDJSON event line"
                            );
                        }
                        return Some((parsed, (body_stream, buf, true)));
                    }
                    return None;
                }

                match body_stream.next().await {
                    Some(Ok(frame)) => {
                        if let Some(data) = frame.data_ref() {
                            buf.extend_from_slice(data);
                        }
                    }
                    Some(Err(e)) => {
                        return Some((
                            Err(anyhow::anyhow!("events body stream error: {e}")),
                            (body_stream, buf, true),
                        ));
                    }
                    None => {
                        done = true;
                    }
                }
            }
        },
    )
}

// ---------------------------------------------------------------------------
// Streaming-body helpers
// ---------------------------------------------------------------------------

/// Convert a hyper response body into an async stream of raw [`Bytes`]
/// chunks, one per body data frame.
///
/// Used by [`DaemonClient::stream_container_logs`] to surface the daemon's
/// response verbatim — no NDJSON parsing, no buffering across frames — so
/// callers receive the same byte sequence the daemon emitted (Docker
/// stdcopy frames in raw mode, NDJSON in JSON mode). Trailers and empty
/// frames are ignored. Body errors are mapped to `anyhow::Error` and
/// terminate the stream.
fn raw_body_stream<B>(body: B) -> impl futures_util::Stream<Item = Result<Bytes>> + Send
where
    B: hyper::body::Body<Data = Bytes> + Send + Unpin + 'static,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    use futures_util::stream::{self, StreamExt as _};
    use http_body_util::BodyStream;

    stream::unfold(BodyStream::new(body), |mut body_stream| async move {
        loop {
            match body_stream.next().await {
                Some(Ok(frame)) => {
                    if let Some(data) = frame.data_ref() {
                        if data.is_empty() {
                            // Empty data frame — keep polling for the next
                            // one (could be a trailers-only frame).
                            continue;
                        }
                        return Some((Ok(data.clone()), body_stream));
                    }
                    // Non-data frame (trailers): fall through to next loop
                    // iteration without producing an item.
                }
                Some(Err(e)) => {
                    return Some((
                        Err(anyhow::anyhow!("logs body stream error: {e}")),
                        body_stream,
                    ));
                }
                None => return None,
            }
        }
    })
}

/// Convert a hyper response body into an async stream of values of type
/// `T`, one per NDJSON line. Generic version of
/// [`parse_ndjson_event_stream`] used by the typed streaming endpoints
/// (`stream_container_stats`, `stream_image_pull`).
///
/// Behaviour:
///   * Empty lines are skipped.
///   * A line that fails JSON-parsing is logged at `warn` (with `kind` as
///     the diagnostic prefix, e.g. `"stats"` / `"pull"`) and yielded as
///     `Err`; the stream continues with the next line.
///   * An IO error on the body is yielded as `Err` and terminates the
///     stream.
///   * On EOF, any trailing bytes without a terminating newline are
///     parsed as one final line.
fn parse_ndjson_typed_stream<B, T>(
    body: B,
    kind: &'static str,
) -> impl futures_util::Stream<Item = Result<T>> + Send
where
    B: hyper::body::Body<Data = Bytes> + Send + Unpin + 'static,
    B::Error: std::error::Error + Send + Sync + 'static,
    T: serde::de::DeserializeOwned + Send + 'static,
{
    use futures_util::stream::{self, StreamExt as _};
    use http_body_util::BodyStream;

    stream::unfold(
        (BodyStream::new(body), Vec::<u8>::new(), false),
        move |(mut body_stream, mut buf, mut done)| async move {
            loop {
                if let Some(idx) = buf.iter().position(|b| *b == b'\n') {
                    let mut line: Vec<u8> = buf.drain(..=idx).collect();
                    line.pop();
                    if line.last() == Some(&b'\r') {
                        line.pop();
                    }
                    if line.is_empty() {
                        continue;
                    }
                    let parsed = serde_json::from_slice::<T>(&line).with_context(|| {
                        format!(
                            "failed to parse NDJSON {kind} line: {}",
                            String::from_utf8_lossy(&line)
                        )
                    });
                    if let Err(ref e) = parsed {
                        tracing::warn!(
                            error = %e,
                            kind = kind,
                            "skipping malformed NDJSON line"
                        );
                    }
                    return Some((parsed, (body_stream, buf, done)));
                }

                if done {
                    if !buf.is_empty() {
                        let line: Vec<u8> = std::mem::take(&mut buf);
                        let parsed = serde_json::from_slice::<T>(&line).with_context(|| {
                            format!(
                                "failed to parse trailing NDJSON {kind} line: {}",
                                String::from_utf8_lossy(&line)
                            )
                        });
                        if let Err(ref e) = parsed {
                            tracing::warn!(
                                error = %e,
                                kind = kind,
                                "skipping malformed trailing NDJSON line"
                            );
                        }
                        return Some((parsed, (body_stream, buf, true)));
                    }
                    return None;
                }

                match body_stream.next().await {
                    Some(Ok(frame)) => {
                        if let Some(data) = frame.data_ref() {
                            buf.extend_from_slice(data);
                        }
                    }
                    Some(Err(e)) => {
                        return Some((
                            Err(anyhow::anyhow!("{kind} body stream error: {e}")),
                            (body_stream, buf, true),
                        ));
                    }
                    None => {
                        done = true;
                    }
                }
            }
        },
    )
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Percent-encode a path segment for safe inclusion in a URL.
///
/// This handles deployment/service names that might contain special characters.
fn urlencoding(s: &str) -> String {
    // For path segments we only need to encode a few characters.
    // Deployment and service names are typically alphanumeric + hyphens,
    // but we handle the general case.
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => out.push(ch),
            _ => {
                for byte in ch.to_string().as_bytes() {
                    use std::fmt::Write;
                    let _ = write!(out, "%{byte:02X}");
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_urlencoding_passthrough() {
        assert_eq!(urlencoding("my-deployment"), "my-deployment");
        assert_eq!(urlencoding("service_v2"), "service_v2");
        assert_eq!(urlencoding("app.web"), "app.web");
    }

    #[test]
    fn test_urlencoding_special_chars() {
        assert_eq!(urlencoding("my app"), "my%20app");
        assert_eq!(urlencoding("a/b"), "a%2Fb");
    }

    #[test]
    fn test_check_status_success() {
        let result = DaemonClient::check_status(hyper::StatusCode::OK, b"{}");
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_status_created() {
        let result = DaemonClient::check_status(hyper::StatusCode::CREATED, b"{}");
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_status_no_content() {
        let result = DaemonClient::check_status(hyper::StatusCode::NO_CONTENT, b"");
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_status_not_found_json() {
        let body = br#"{"error":"Deployment 'foo' not found"}"#;
        let result = DaemonClient::check_status(hyper::StatusCode::NOT_FOUND, body);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("404"));
        assert!(msg.contains("Deployment 'foo' not found"));
    }

    #[test]
    fn test_check_status_error_plain_text() {
        let body = b"something went wrong";
        let result = DaemonClient::check_status(hyper::StatusCode::INTERNAL_SERVER_ERROR, body);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("500"));
        assert!(msg.contains("something went wrong"));
    }

    #[test]
    fn test_parse_json_valid() {
        let body = br#"{"name":"test","status":"running"}"#;
        let val: serde_json::Value = DaemonClient::parse_json(body).unwrap();
        assert_eq!(val["name"], "test");
        assert_eq!(val["status"], "running");
    }

    #[test]
    fn test_parse_json_invalid() {
        let body = b"not json";
        let result: Result<serde_json::Value> = DaemonClient::parse_json(body);
        assert!(result.is_err());
    }

    #[test]
    fn container_logs_cli_decodes_plain_text() {
        let body: &[u8] = b"[stdout] hello\n[stderr] world\n";
        let out = DaemonClient::decode_text_body(body).expect("should decode");
        assert_eq!(out, "[stdout] hello\n[stderr] world\n");
    }

    #[test]
    fn container_logs_cli_rejects_invalid_utf8() {
        // 0xFF is not valid UTF-8
        let bad: &[u8] = &[0xFF, 0xFE, 0xFD];
        assert!(DaemonClient::decode_text_body(bad).is_err());
    }

    #[test]
    fn test_find_self_binary() {
        // This test verifies the logic works without panicking.
        // current_exe() should always succeed in a test runner.
        let result = DaemonClient::find_self_binary();
        assert!(result.is_ok());
    }

    /// Round-trip the wire shape exercised by [`DaemonClient::create_container`]:
    ///
    /// 1. Serialize a populated [`CreateContainerRequest`] the way the method
    ///    serializes its body before `POST /api/v1/containers` — this verifies
    ///    the request struct's `Serialize` impl reaches every field the
    ///    Docker-compat shim cares about (image, name, env, command, labels,
    ///    ports).
    /// 2. Parse a sample daemon response — modelled after the
    ///    `ContainerInfo`-shaped body the handler returns on `201 Created`,
    ///    plus an optional `warnings` array that future daemon builds may
    ///    surface — through [`DaemonClient::parse_json`], the same path the
    ///    method itself takes after `post_json` returns.
    ///
    /// We exercise [`DaemonClient::parse_json`] (the static helper the
    /// method delegates to) rather than spinning up a Unix-socket-backed mock
    /// server, because [`DaemonClient`]'s connector is hard-wired to UDS on
    /// Unix / TCP-loopback on Windows and does not accept arbitrary base URLs
    /// — wiremock-style tests here would require a UDS-bound HTTP server,
    /// which is far out of scope for this additive client method.
    #[test]
    fn create_container_serializes_request_and_parses_response() {
        use std::collections::HashMap;
        use zlayer_types::api::containers::CreateContainerRequest;
        use zlayer_types::spec::PortMapping;

        // 1) Request serialization — what the method puts on the wire.
        let mut env = HashMap::new();
        env.insert("RUST_LOG".to_string(), "info".to_string());

        let mut labels = HashMap::new();
        labels.insert("com.docker.compose.project".to_string(), "demo".to_string());

        let request = CreateContainerRequest {
            image: "nginx:1.25".to_string(),
            name: Some("web-1".to_string()),
            pull_policy: Some("if_not_present".to_string()),
            env,
            command: Some(vec!["nginx".into(), "-g".into(), "daemon off;".into()]),
            labels,
            ports: vec![PortMapping {
                container_port: 80,
                host_port: Some(8080),
                protocol: zlayer_types::spec::PortProtocol::Tcp,
                host_ip: String::new(),
            }],
            ..CreateContainerRequest::default()
        };

        let body = serde_json::to_string(&request).expect("CreateContainerRequest must serialize");
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["image"], "nginx:1.25");
        assert_eq!(parsed["name"], "web-1");
        assert_eq!(parsed["pull_policy"], "if_not_present");
        assert_eq!(parsed["env"]["RUST_LOG"], "info");
        assert_eq!(parsed["command"][0], "nginx");
        assert_eq!(parsed["ports"][0]["container_port"], 80);
        assert_eq!(parsed["ports"][0]["host_port"], 8080);

        // 2) Response parsing — what the method reads off `post_json`'s body.
        // The daemon returns the full `ContainerInfo` document; only the
        // fields on `CreateContainerResponse` should be required.
        let daemon_body = br#"{
            "id": "ctr_abc123",
            "name": "web-1",
            "image": "nginx:1.25",
            "state": "running",
            "labels": {},
            "created_at": "2026-05-03T12:00:00Z",
            "warnings": ["image pulled with cached digest"]
        }"#;
        let resp: CreateContainerResponse =
            DaemonClient::parse_json(daemon_body).expect("daemon ContainerInfo must parse");
        assert_eq!(resp.id, "ctr_abc123");
        assert_eq!(resp.name.as_deref(), Some("web-1"));
        assert_eq!(resp.warnings, vec!["image pulled with cached digest"]);

        // The shape older daemon builds emit (no `warnings` field) must also
        // parse — `warnings` defaults to an empty Vec, `name` to None.
        let legacy_body = br#"{
            "id": "ctr_xyz",
            "image": "nginx:1.25",
            "state": "running",
            "labels": {},
            "created_at": "2026-05-03T12:00:00Z"
        }"#;
        let legacy: CreateContainerResponse =
            DaemonClient::parse_json(legacy_body).expect("legacy ContainerInfo must parse");
        assert_eq!(legacy.id, "ctr_xyz");
        assert!(legacy.warnings.is_empty());
        assert!(legacy.name.is_none());
    }

    // ----------------------------------------------------------------------
    // Volume / bridge-network / wait helpers — wire-shape round-trips.
    //
    // These mirror the [`create_container_serializes_request_and_parses_response`]
    // test pattern: each asserts that the request struct serializes to the
    // exact JSON the daemon expects, and that a hand-written sample of the
    // daemon's response document parses through [`DaemonClient::parse_json`]
    // — the same static helper the live methods delegate to.
    //
    // We do NOT spin up a wiremock server: [`DaemonClient`]'s connector is
    // hard-wired to UDS on Unix / TCP-loopback on Windows and does not accept
    // arbitrary base URLs, so a mock-server test would need a UDS-bound HTTP
    // server. That infrastructure does not exist in this crate today, so we
    // exercise the same parse/serialize seams the wire path uses.
    // ----------------------------------------------------------------------

    #[test]
    fn inspect_volume_round_trip() {
        // Response-only path: GET /api/v1/volumes/{name} returns a
        // VolumeInfo document; the helper unwraps it through parse_json.
        let daemon_body = br#"{
            "name": "pg-data",
            "path": "/var/lib/zlayer/volumes/pg-data",
            "size_bytes": 4096,
            "labels": {"app": "postgres"},
            "created_at": "2026-05-03T12:00:00Z",
            "in_use_by": ["ctr_abc"]
        }"#;
        let info: zlayer_types::api::volumes::VolumeInfo =
            DaemonClient::parse_json(daemon_body).expect("VolumeInfo must parse");
        assert_eq!(info.name, "pg-data");
        assert_eq!(info.path, "/var/lib/zlayer/volumes/pg-data");
        assert_eq!(info.size_bytes, Some(4096));
        assert_eq!(info.labels.get("app").map(String::as_str), Some("postgres"));
        assert_eq!(info.created_at, "2026-05-03T12:00:00Z");
        assert_eq!(info.in_use_by, vec!["ctr_abc".to_string()]);
    }

    #[test]
    fn create_volume_round_trip() {
        use std::collections::HashMap;
        use zlayer_types::api::volumes::{CreateVolumeRequest, VolumeInfo};

        // 1) Request serialization.
        let labels = HashMap::from([("env".to_string(), "prod".to_string())]);
        let request = CreateVolumeRequest {
            name: "pg-data".to_string(),
            size: Some("10Gi".to_string()),
            tier: Some("local".to_string()),
            labels: Some(labels),
        };
        let body = serde_json::to_string(&request).expect("CreateVolumeRequest must serialize");
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["name"], "pg-data");
        assert_eq!(parsed["size"], "10Gi");
        assert_eq!(parsed["tier"], "local");
        assert_eq!(parsed["labels"]["env"], "prod");

        // 2) Response parsing — what `POST /api/v1/volumes` returns on 201.
        let daemon_body = br#"{
            "name": "pg-data",
            "path": "/var/lib/zlayer/volumes/pg-data",
            "size_bytes": 0,
            "labels": {"env": "prod"},
            "created_at": "2026-05-03T12:00:00Z"
        }"#;
        let info: VolumeInfo =
            DaemonClient::parse_json(daemon_body).expect("created VolumeInfo must parse");
        assert_eq!(info.name, "pg-data");
        assert_eq!(info.size_bytes, Some(0));
        assert!(info.in_use_by.is_empty());
    }

    #[test]
    fn list_bridge_networks_round_trip() {
        // GET /api/v1/container-networks returns Vec<BridgeNetwork>. The
        // wire shape uses chrono's RFC3339 default for `created_at`.
        let daemon_body = br#"[
            {
                "id": "11111111-1111-4111-8111-111111111111",
                "name": "alpha",
                "driver": "bridge",
                "subnet": "10.240.0.0/24",
                "labels": {"env": "dev"},
                "internal": false,
                "created_at": "2026-05-03T12:00:00Z"
            },
            {
                "id": "22222222-2222-4222-8222-222222222222",
                "name": "beta",
                "driver": "overlay",
                "labels": {},
                "internal": true,
                "created_at": "2026-05-03T12:01:00Z"
            }
        ]"#;
        let nets: Vec<zlayer_types::spec::BridgeNetwork> =
            DaemonClient::parse_json(daemon_body).expect("Vec<BridgeNetwork> must parse");
        assert_eq!(nets.len(), 2);
        assert_eq!(nets[0].name, "alpha");
        assert_eq!(
            nets[0].driver,
            zlayer_types::spec::BridgeNetworkDriver::Bridge
        );
        assert_eq!(nets[0].subnet.as_deref(), Some("10.240.0.0/24"));
        assert!(!nets[0].internal);
        assert_eq!(nets[1].name, "beta");
        assert_eq!(
            nets[1].driver,
            zlayer_types::spec::BridgeNetworkDriver::Overlay
        );
        assert!(nets[1].internal);
    }

    #[test]
    fn create_bridge_network_round_trip() {
        use std::collections::HashMap;
        use zlayer_types::api::container_networks::CreateBridgeNetworkRequest;
        use zlayer_types::spec::{BridgeNetwork, BridgeNetworkDriver};

        // 1) Request serialization.
        let labels = HashMap::from([("env".to_string(), "prod".to_string())]);
        let request = CreateBridgeNetworkRequest {
            name: "edge".to_string(),
            driver: Some(BridgeNetworkDriver::Bridge),
            subnet: Some("10.241.0.0/24".to_string()),
            labels,
            internal: false,
        };
        let body =
            serde_json::to_string(&request).expect("CreateBridgeNetworkRequest must serialize");
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["name"], "edge");
        assert_eq!(parsed["driver"], "bridge");
        assert_eq!(parsed["subnet"], "10.241.0.0/24");
        assert_eq!(parsed["labels"]["env"], "prod");
        assert_eq!(parsed["internal"], false);

        // 2) Response parsing — what `POST /api/v1/container-networks`
        //    returns on 201.
        let daemon_body = br#"{
            "id": "33333333-3333-4333-8333-333333333333",
            "name": "edge",
            "driver": "bridge",
            "subnet": "10.241.0.0/24",
            "labels": {"env": "prod"},
            "internal": false,
            "created_at": "2026-05-03T12:00:00Z"
        }"#;
        let net: BridgeNetwork =
            DaemonClient::parse_json(daemon_body).expect("created BridgeNetwork must parse");
        assert_eq!(net.name, "edge");
        assert_eq!(net.driver, BridgeNetworkDriver::Bridge);
        assert_eq!(net.subnet.as_deref(), Some("10.241.0.0/24"));
        assert!(!net.internal);
    }

    #[test]
    fn get_bridge_network_round_trip() {
        // GET /api/v1/container-networks/{id_or_name} returns
        // BridgeNetworkDetails (a flattened BridgeNetwork plus
        // attached_containers). The helper deserializes only into
        // BridgeNetwork; the extra `attached_containers` field is ignored
        // by serde.
        let daemon_body = br#"{
            "id": "44444444-4444-4444-8444-444444444444",
            "name": "edge",
            "driver": "bridge",
            "labels": {},
            "internal": false,
            "created_at": "2026-05-03T12:00:00Z",
            "attached_containers": [
                {
                    "container_id": "ctr_abc",
                    "container_name": "web",
                    "aliases": ["web", "frontend"],
                    "ipv4": "10.241.0.5"
                }
            ]
        }"#;
        let net: zlayer_types::spec::BridgeNetwork =
            DaemonClient::parse_json(daemon_body).expect("BridgeNetwork must parse");
        assert_eq!(net.name, "edge");
        assert_eq!(net.driver, zlayer_types::spec::BridgeNetworkDriver::Bridge);
        // attached_containers is not on BridgeNetwork — it's on
        // BridgeNetworkDetails — so it's silently ignored by this parser.
    }

    #[test]
    fn delete_bridge_network_round_trip() {
        // The delete helper inspects only the HTTP status, so we exercise
        // the only piece of body parsing it does: the 4xx error path.
        // 204 -> Ok(true); 404 -> Ok(false); other 4xx/5xx -> Err.
        let body = br#"{"error":"Bridge network 'foo' has 1 attached container(s); pass ?force=true to delete anyway"}"#;
        let err = DaemonClient::check_status(hyper::StatusCode::CONFLICT, body)
            .expect_err("409 must surface an error");
        let msg = err.to_string();
        assert!(
            msg.contains("409"),
            "expected status code in message: {msg}"
        );
        assert!(msg.contains("attached"), "expected error context: {msg}");
    }

    #[test]
    fn connect_container_to_bridge_network_round_trip() {
        use zlayer_types::api::container_networks::ConnectBridgeNetworkRequest;

        // The helper builds a `ConnectBridgeNetworkRequest` with the given
        // container id and aliases, leaves `ipv4_address` unset, and POSTs
        // it. Verify the JSON body matches what the daemon's
        // `connect_container_network` handler expects.
        let request = ConnectBridgeNetworkRequest {
            container_id: "ctr_abc".to_string(),
            aliases: vec!["web".to_string(), "frontend".to_string()],
            ipv4_address: None,
        };
        let body =
            serde_json::to_string(&request).expect("ConnectBridgeNetworkRequest must serialize");
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["container_id"], "ctr_abc");
        assert_eq!(parsed["aliases"][0], "web");
        assert_eq!(parsed["aliases"][1], "frontend");
        assert!(
            parsed.get("ipv4_address").is_none(),
            "ipv4_address must be skipped when None: {body}"
        );
    }

    #[test]
    fn disconnect_container_from_bridge_network_round_trip() {
        use zlayer_types::api::container_networks::DisconnectBridgeNetworkRequest;

        let request = DisconnectBridgeNetworkRequest {
            container_id: "ctr_abc".to_string(),
            force: true,
        };
        let body =
            serde_json::to_string(&request).expect("DisconnectBridgeNetworkRequest must serialize");
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["container_id"], "ctr_abc");
        assert_eq!(parsed["force"], true);
    }

    #[test]
    fn wait_container_round_trip() {
        // Response parsing — Docker's wait shape: status_code + optional
        // error envelope. Both the success (no error) and failure (error
        // populated) shapes must round-trip through parse_json.
        let success_body = br#"{"status_code": 0}"#;
        let resp: WaitContainerResponse =
            DaemonClient::parse_json(success_body).expect("clean wait response must parse");
        assert_eq!(resp.status_code, 0);
        assert!(resp.error.is_none());

        let signal_body = br#"{"status_code": 137}"#;
        let resp: WaitContainerResponse =
            DaemonClient::parse_json(signal_body).expect("signal-killed wait response must parse");
        assert_eq!(resp.status_code, 137);
        assert!(resp.error.is_none());

        let error_body = br#"{
            "status_code": -1,
            "error": {"message": "container removed before reaching not-running"}
        }"#;
        let resp: WaitContainerResponse =
            DaemonClient::parse_json(error_body).expect("errored wait response must parse");
        assert_eq!(resp.status_code, -1);
        let err = resp.error.expect("error envelope must be present");
        assert_eq!(err.message, "container removed before reaching not-running");
    }

    // ----------------------------------------------------------------------
    // events_stream — URL building + NDJSON parse loop.
    //
    // These tests exercise the real NDJSON parser used by
    // `DaemonClient::events_stream`. We feed it a hyper `Body`-shaped
    // input ("an in-memory server returning two NDJSON lines") and assert
    // the stream yields two parsed `DaemonEvent`s, plus exercise the
    // skip-malformed-line path so a single bad line doesn't kill the
    // whole subscription. Spinning up a UDS-backed daemon would re-test
    // hyper's transport rather than this method's logic.
    // ----------------------------------------------------------------------

    #[test]
    fn events_path_no_filters() {
        let path = build_events_path(true, &[]);
        assert_eq!(path, "/api/v1/events?follow=true");

        let path = build_events_path(false, &[]);
        assert_eq!(path, "/api/v1/events?follow=false");
    }

    #[test]
    fn events_path_with_label_filters_url_encoded() {
        let filters = vec![
            ("app".to_string(), "web".to_string()),
            ("env".to_string(), "prod".to_string()),
        ];
        let path = build_events_path(true, &filters);
        assert_eq!(
            path,
            "/api/v1/events?follow=true&label=app=web&label=env=prod"
        );

        // Spaces and slashes in keys/values must be percent-encoded so the
        // query string remains a single hop.
        let filters = vec![("com.example/team".to_string(), "platform infra".to_string())];
        let path = build_events_path(false, &filters);
        assert_eq!(
            path,
            "/api/v1/events?follow=false&label=com.example%2Fteam=platform%20infra"
        );
    }

    /// In-memory body adapter used by [`events_stream_yields_two_parsed_events`].
    ///
    /// Wraps a `Vec<Bytes>` queue as a hyper `Body` so we can feed the
    /// NDJSON parser exactly the chunks an `axum::Body::from_stream` body
    /// would deliver — including chunk boundaries that split a single
    /// line — without binding a real socket.
    struct ChunkedBody {
        chunks: std::collections::VecDeque<Bytes>,
    }

    impl ChunkedBody {
        fn new<I: IntoIterator<Item = Bytes>>(chunks: I) -> Self {
            Self {
                chunks: chunks.into_iter().collect(),
            }
        }
    }

    impl hyper::body::Body for ChunkedBody {
        type Data = Bytes;
        type Error = std::convert::Infallible;

        fn poll_frame(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<std::result::Result<hyper::body::Frame<Self::Data>, Self::Error>>>
        {
            match self.chunks.pop_front() {
                Some(b) => std::task::Poll::Ready(Some(Ok(hyper::body::Frame::data(b)))),
                None => std::task::Poll::Ready(None),
            }
        }
    }

    #[tokio::test]
    async fn events_stream_yields_two_parsed_events() {
        use futures_util::StreamExt as _;

        // Two valid NDJSON DaemonEvent lines, mirroring what the
        // `GET /api/v1/events` handler emits via `ndjson_line`. Split
        // across three chunks so we also exercise mid-line buffering.
        let body = ChunkedBody::new(vec![
            Bytes::from_static(
                b"{\"resource\":\"container\",\"kind\":\"start\",\"id\":\"c1\",\"at\":\"2026-05-03T12:00:00Z\"}\n",
            ),
            Bytes::from_static(b"{\"resource\":\"image\",\"kind\":\"pull\","),
            Bytes::from_static(
                b"\"reference\":\"nginx:latest\",\"at\":\"2026-05-03T12:00:01Z\"}\n",
            ),
        ]);

        let mut stream = Box::pin(parse_ndjson_event_stream(body));

        let first = stream
            .next()
            .await
            .expect("first event present")
            .expect("first event parses");
        match first {
            zlayer_api::DaemonEvent::Container(c) => {
                assert_eq!(c.id, "c1");
                assert_eq!(c.kind, zlayer_api::ContainerEventKind::Start);
            }
            other => panic!("expected container event, got {other:?}"),
        }

        let second = stream
            .next()
            .await
            .expect("second event present")
            .expect("second event parses");
        match second {
            zlayer_api::DaemonEvent::Image(i) => {
                assert_eq!(i.reference, "nginx:latest");
                assert_eq!(i.kind, zlayer_api::ImageEventKind::Pull);
            }
            other => panic!("expected image event, got {other:?}"),
        }

        // Stream ends cleanly after the body is exhausted.
        assert!(stream.next().await.is_none(), "stream should end on EOF");
    }

    #[tokio::test]
    async fn events_stream_skips_malformed_line_without_terminating() {
        use futures_util::StreamExt as _;

        // A bad line wedged between two valid ones must surface as an
        // `Err` item but the parser must keep reading the next line.
        let body = ChunkedBody::new(vec![
            Bytes::from_static(
                b"{\"resource\":\"container\",\"kind\":\"start\",\"id\":\"c1\",\"at\":\"2026-05-03T12:00:00Z\"}\n",
            ),
            Bytes::from_static(b"this is not json\n"),
            Bytes::from_static(
                b"{\"resource\":\"image\",\"kind\":\"pull\",\"reference\":\"alpine:3\",\"at\":\"2026-05-03T12:00:01Z\"}\n",
            ),
        ]);

        let mut stream = Box::pin(parse_ndjson_event_stream(body));

        let first = stream.next().await.expect("first item present");
        first.expect("first line parses cleanly");

        let bad = stream.next().await.expect("bad line surfaces");
        assert!(bad.is_err(), "malformed line must yield Err");

        let third = stream.next().await.expect("third item present");
        let ev = third.expect("recovery line parses cleanly");
        match ev {
            zlayer_api::DaemonEvent::Image(i) => assert_eq!(i.reference, "alpine:3"),
            other => panic!("expected image event after recovery, got {other:?}"),
        }
    }

    // ----------------------------------------------------------------------
    // stream_container_logs / stream_container_stats / stream_image_pull —
    // wire-format parsing.
    //
    // These tests exercise the helpers used by the streaming methods
    // (`raw_body_stream`, `parse_ndjson_typed_stream`) against the same
    // `ChunkedBody` mock used by the events_stream tests above. We don't
    // bind a UDS-backed mock daemon — that would re-test hyper's transport
    // layer rather than the parsing logic this PR adds — so the public
    // `DaemonClient::stream_*` methods are covered indirectly through
    // their underlying helpers, which is the only logic the client owns.
    // ----------------------------------------------------------------------

    #[tokio::test]
    async fn stream_container_logs_yields_raw_body_chunks_unchanged() {
        use futures_util::StreamExt as _;

        // Three data frames mirroring what the `?format=raw` Docker-compat
        // path emits (stdcopy-framed bytes). The helper must surface each
        // frame as a single `Bytes` item without re-buffering or parsing.
        let body = ChunkedBody::new(vec![
            Bytes::from_static(b"\x01\x00\x00\x00\x00\x00\x00\x05hello"),
            Bytes::from_static(b"\x02\x00\x00\x00\x00\x00\x00\x05world"),
            Bytes::from_static(b"\x01\x00\x00\x00\x00\x00\x00\x03end"),
        ]);

        let mut stream = Box::pin(raw_body_stream(body));

        let first = stream
            .next()
            .await
            .expect("first chunk present")
            .expect("first chunk ok");
        assert_eq!(first.as_ref(), b"\x01\x00\x00\x00\x00\x00\x00\x05hello");

        let second = stream
            .next()
            .await
            .expect("second chunk present")
            .expect("second chunk ok");
        assert_eq!(second.as_ref(), b"\x02\x00\x00\x00\x00\x00\x00\x05world");

        let third = stream
            .next()
            .await
            .expect("third chunk present")
            .expect("third chunk ok");
        assert_eq!(third.as_ref(), b"\x01\x00\x00\x00\x00\x00\x00\x03end");

        assert!(stream.next().await.is_none(), "stream ends on EOF");
    }

    #[tokio::test]
    async fn stream_container_stats_parses_two_ndjson_samples() {
        use futures_util::StreamExt as _;

        // Two valid NDJSON StatsSample lines split mid-line across three
        // chunks so we also exercise the buffering path.
        let body = ChunkedBody::new(vec![
            Bytes::from_static(
                b"{\"cpu_total_ns\":100,\"cpu_system_ns\":1000,\"online_cpus\":2,\
                  \"mem_used_bytes\":1024,\"mem_limit_bytes\":2048,\
                  \"net_rx_bytes\":10,\"net_tx_bytes\":20,\
                  \"blkio_read_bytes\":30,\"blkio_write_bytes\":40,\
                  \"pids_current\":5,\"pids_limit\":100,\
                  \"timestamp\":\"2026-05-03T12:00:00Z\"}\n",
            ),
            Bytes::from_static(
                b"{\"cpu_total_ns\":200,\"cpu_system_ns\":2000,\"online_cpus\":2,\
                  \"mem_used_bytes\":2048,\"mem_limit_bytes\":2048,\
                  \"net_rx_bytes\":11,\"net_tx_bytes\":21,",
            ),
            Bytes::from_static(
                b"\"blkio_read_bytes\":31,\"blkio_write_bytes\":41,\
                  \"pids_current\":6,\
                  \"timestamp\":\"2026-05-03T12:00:01Z\"}\n",
            ),
        ]);

        let mut stream = Box::pin(parse_ndjson_typed_stream::<_, StatsSample>(body, "stats"));

        let first = stream
            .next()
            .await
            .expect("first sample present")
            .expect("first sample parses");
        assert_eq!(first.cpu_total_ns, 100);
        assert_eq!(first.online_cpus, 2);
        assert_eq!(first.mem_used_bytes, 1024);
        assert_eq!(first.pids_limit, Some(100));

        let second = stream
            .next()
            .await
            .expect("second sample present")
            .expect("second sample parses");
        assert_eq!(second.cpu_total_ns, 200);
        assert_eq!(second.mem_used_bytes, 2048);
        // `pids_limit` is missing on this line; default skip means `None`.
        assert_eq!(second.pids_limit, None);

        assert!(stream.next().await.is_none(), "stream ends on EOF");
    }

    #[tokio::test]
    async fn stream_container_stats_skips_blank_and_recovers_from_bad_line() {
        use futures_util::StreamExt as _;

        // One blank line + one malformed line wedged between two valid
        // samples. The blank line must be skipped silently; the malformed
        // line must yield an `Err` item without ending the stream.
        let body = ChunkedBody::new(vec![
            Bytes::from_static(b"\n"),
            Bytes::from_static(
                b"{\"cpu_total_ns\":1,\"cpu_system_ns\":2,\"online_cpus\":1,\
                  \"mem_used_bytes\":3,\"mem_limit_bytes\":4,\
                  \"net_rx_bytes\":0,\"net_tx_bytes\":0,\
                  \"blkio_read_bytes\":0,\"blkio_write_bytes\":0,\
                  \"pids_current\":1,\
                  \"timestamp\":\"2026-05-03T12:00:00Z\"}\n",
            ),
            Bytes::from_static(b"this is not json\n"),
            Bytes::from_static(
                b"{\"cpu_total_ns\":9,\"cpu_system_ns\":99,\"online_cpus\":1,\
                  \"mem_used_bytes\":3,\"mem_limit_bytes\":4,\
                  \"net_rx_bytes\":0,\"net_tx_bytes\":0,\
                  \"blkio_read_bytes\":0,\"blkio_write_bytes\":0,\
                  \"pids_current\":1,\
                  \"timestamp\":\"2026-05-03T12:00:01Z\"}\n",
            ),
        ]);

        let mut stream = Box::pin(parse_ndjson_typed_stream::<_, StatsSample>(body, "stats"));

        let first = stream
            .next()
            .await
            .expect("first sample present")
            .expect("first sample parses");
        assert_eq!(first.cpu_total_ns, 1);

        let bad = stream.next().await.expect("bad line surfaces");
        assert!(bad.is_err(), "malformed line must yield Err");

        let third = stream
            .next()
            .await
            .expect("recovery sample present")
            .expect("recovery sample parses");
        assert_eq!(third.cpu_total_ns, 9);

        assert!(stream.next().await.is_none(), "stream ends on EOF");
    }

    #[tokio::test]
    async fn stream_image_pull_parses_status_and_done_variants() {
        use futures_util::StreamExt as _;

        // One Status line for an in-flight layer + one terminal Done line.
        // Verifies the `tag = "type"` enum representation deserializes both
        // variants from a single NDJSON parser.
        let body = ChunkedBody::new(vec![
            Bytes::from_static(
                b"{\"type\":\"status\",\"id\":\"sha256:abc\",\
                  \"status\":\"Downloading\",\
                  \"progress\":\"[==>      ] 1.2MB/4.0MB\",\
                  \"current\":1200000,\"total\":4000000}\n",
            ),
            Bytes::from_static(
                b"{\"type\":\"done\",\"reference\":\"docker.io/library/alpine:3\",\
                  \"digest\":\"sha256:deadbeef\"}\n",
            ),
        ]);

        let mut stream = Box::pin(parse_ndjson_typed_stream::<_, PullProgress>(body, "pull"));

        let first = stream
            .next()
            .await
            .expect("first event present")
            .expect("first event parses");
        match first {
            PullProgress::Status {
                id,
                status,
                progress,
                current,
                total,
            } => {
                assert_eq!(id.as_deref(), Some("sha256:abc"));
                assert_eq!(status, "Downloading");
                assert!(progress.is_some());
                assert_eq!(current, Some(1_200_000));
                assert_eq!(total, Some(4_000_000));
            }
            PullProgress::Done { .. } => panic!("expected Status, got Done"),
        }

        let second = stream
            .next()
            .await
            .expect("second event present")
            .expect("second event parses");
        match second {
            PullProgress::Done { reference, digest } => {
                assert_eq!(reference, "docker.io/library/alpine:3");
                assert_eq!(digest.as_deref(), Some("sha256:deadbeef"));
            }
            PullProgress::Status { .. } => panic!("expected Done, got Status"),
        }

        assert!(stream.next().await.is_none(), "stream ends on EOF");
    }

    // ----------------------------------------------------------------------
    // Exec instances — wire shapes + WebSocket session loop.
    //
    // The HTTP CRUD methods (`create_exec`, `inspect_exec`, `resize_exec`,
    // `resize_container`) reuse the existing `post_json` / `get` helpers,
    // so testing them end-to-end would re-exercise the same code paths the
    // earlier `check_status` / `parse_json` tests cover. We instead pin
    // down the wire shapes (round-trip the JSON bodies the daemon
    // produces / consumes) and exercise the only piece of bespoke logic
    // this PR adds — the `run_exec_pty_session` loop driving the
    // bidirectional WebSocket — against an in-process mock server built
    // on `tokio::io::duplex`.
    // ----------------------------------------------------------------------

    #[test]
    fn exec_options_serializes_with_snake_case_fields() {
        let opts = ExecOptions {
            command: vec!["sh".to_string(), "-c".to_string(), "echo hi".to_string()],
            env: vec!["FOO=bar".to_string()],
            working_dir: Some("/srv".to_string()),
            user: Some("nobody".to_string()),
            privileged: false,
            tty: true,
            attach_stdin: true,
            attach_stdout: true,
            attach_stderr: false,
        };
        let json = serde_json::to_value(&opts).expect("serialize ExecOptions");
        assert_eq!(json["command"], serde_json::json!(["sh", "-c", "echo hi"]));
        assert_eq!(json["env"], serde_json::json!(["FOO=bar"]));
        assert_eq!(json["working_dir"], "/srv");
        assert_eq!(json["user"], "nobody");
        assert_eq!(json["privileged"], false);
        assert_eq!(json["tty"], true);
        assert_eq!(json["attach_stdin"], true);
        assert_eq!(json["attach_stdout"], true);
        assert_eq!(json["attach_stderr"], false);

        // Round-trip: deserializing the produced JSON yields the same
        // struct, with `Option`s preserved.
        let back: ExecOptions = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, opts);

        // Optional fields elide on the wire when `None`.
        let minimal = ExecOptions {
            command: vec!["true".to_string()],
            ..Default::default()
        };
        let v = serde_json::to_value(&minimal).expect("serialize minimal");
        assert!(v.get("working_dir").is_none(), "None working_dir omitted");
        assert!(v.get("user").is_none(), "None user omitted");
    }

    #[test]
    fn exec_instance_json_deserializes_daemon_wire_shape() {
        // Mirrors the JSON shape `ExecInstance` would produce if serialized
        // verbatim from the daemon's storage layer
        // (`crates/zlayer-api/src/handlers/exec_instances.rs::ExecInstance`).
        // Lifecycle fields are present-but-null pre-start; absent fields
        // also deserialize cleanly thanks to the `serde(default)` attrs.
        let body = serde_json::json!({
            "id": "0".repeat(64),
            "container_id": { "service": "web", "replica": 1 },
            "options": {
                "command": ["sh"],
                "env": [],
                "working_dir": null,
                "user": null,
                "privileged": false,
                "tty": true,
                "attach_stdin": true,
                "attach_stdout": true,
                "attach_stderr": true,
            },
            "created_at": "2026-05-03T12:00:00Z",
            "started_at": null,
            "finished_at": null,
            "exit_code": null,
        });
        let parsed: ExecInstanceJson =
            serde_json::from_value(body).expect("ExecInstanceJson parse");
        assert_eq!(parsed.id.len(), 64);
        assert_eq!(parsed.container_id.service, "web");
        assert_eq!(parsed.container_id.replica, 1);
        assert!(parsed.options.tty);
        assert!(parsed.started_at.is_none());
        assert!(parsed.exit_code.is_none());

        // A finished exec carries timestamps + an exit code; verify the
        // optional fields populate.
        let finished = serde_json::json!({
            "id": "a".repeat(64),
            "container_id": { "service": "svc", "replica": 0 },
            "options": { "command": ["true"] },
            "created_at": "2026-05-03T12:00:00Z",
            "started_at": "2026-05-03T12:00:01Z",
            "finished_at": "2026-05-03T12:00:02Z",
            "exit_code": 7,
        });
        let parsed2: ExecInstanceJson = serde_json::from_value(finished).expect("finished parse");
        assert_eq!(parsed2.exit_code, Some(7));
        assert!(parsed2.started_at.is_some());
        assert!(parsed2.finished_at.is_some());
    }

    #[test]
    fn create_exec_response_accepts_id_and_capital_id_alias() {
        let lower: CreateExecResponse =
            serde_json::from_value(serde_json::json!({ "id": "deadbeef" })).expect("lowercase");
        assert_eq!(lower.id, "deadbeef");

        let docker: CreateExecResponse =
            serde_json::from_value(serde_json::json!({ "Id": "cafef00d" })).expect("Docker case");
        assert_eq!(docker.id, "cafef00d");
    }

    /// Drive `run_exec_pty_session` end-to-end against an in-process WS
    /// peer talking over a `tokio::io::duplex()` pipe. Asserts:
    ///
    /// 1. Binary stdout frames from the server flow through
    ///    `ExecPtyConnection::reader` unchanged.
    /// 2. Stdin payloads sent on `ExecPtyWriter::send_stdin` arrive on the
    ///    server as binary frames.
    /// 3. Resize hints sent on the resize channel arrive on the server as
    ///    a JSON `{"resize":{"rows":..,"cols":..}}` text frame.
    /// 4. A normal-closure (1000) close frame with a numeric reason
    ///    resolves the exit future to that integer.
    #[tokio::test]
    async fn exec_pty_session_round_trips_binary_resize_and_exit_code() {
        use futures_util::{SinkExt as _, StreamExt as _};
        use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
        use tokio_tungstenite::tungstenite::protocol::CloseFrame;
        use tokio_tungstenite::tungstenite::Message as WsMessage;

        // 64 KiB duplex buffer is plenty for a few-frame test exchange.
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);

        // Server side: drive `accept_async` directly on the duplex half.
        // This skips HTTP-upgrade handshake validation but keeps the WS
        // framing identical to the real server.
        let server_task = tokio::spawn(async move {
            let mut server = tokio_tungstenite::accept_async(server_io)
                .await
                .expect("server handshake");
            // Push one binary frame the client must surface unchanged.
            server
                .send(WsMessage::Binary(Bytes::from_static(b"hello")))
                .await
                .expect("server send stdout");

            // Receive one stdin binary frame + one resize text frame from
            // the client and stash them for the assertions below.
            let stdin = server
                .next()
                .await
                .expect("server got msg")
                .expect("server msg ok");
            let resize = server
                .next()
                .await
                .expect("server got resize")
                .expect("server resize ok");

            // Send a normal-close with the exit code in the reason field.
            server
                .send(WsMessage::Close(Some(CloseFrame {
                    code: CloseCode::Normal,
                    reason: "42".into(),
                })))
                .await
                .expect("server send close");

            (stdin, resize)
        });

        // Client side: handshake against the duplex peer, then wire the
        // resulting WebSocketStream into `run_exec_pty_session` with the
        // same channels `start_exec_pty` would have built.
        let (client, _resp) =
            tokio_tungstenite::client_async("ws://localhost/api/v1/exec/abc/start", client_io)
                .await
                .expect("client handshake");

        let (out_tx, out_rx) = tokio::sync::mpsc::channel::<ExecPtyOutbound>(8);
        let (read_tx, read_rx) = tokio::sync::mpsc::channel::<Result<Bytes>>(8);
        let (exit_tx, exit_rx) = tokio::sync::oneshot::channel::<Result<Option<i32>>>();
        tokio::spawn(super::run_exec_pty_session(
            client, out_rx, read_tx, exit_tx,
        ));

        // 1. Binary frame: forwarded as-is.
        let mut reader = tokio_stream::wrappers::ReceiverStream::new(read_rx);
        let first = reader
            .next()
            .await
            .expect("first chunk")
            .expect("first chunk ok");
        assert_eq!(first.as_ref(), b"hello");

        // 2. Stdin: client writes "input" -> server should see binary frame.
        out_tx
            .send(ExecPtyOutbound::Stdin(Bytes::from_static(b"input")))
            .await
            .expect("queue stdin");

        // 3. Resize: client sends (24, 80) -> server should see a JSON
        //    text frame matching the documented control shape.
        out_tx
            .send(ExecPtyOutbound::Resize { rows: 24, cols: 80 })
            .await
            .expect("queue resize");

        let (stdin_msg, resize_msg) = server_task.await.expect("server task joined");
        match stdin_msg {
            WsMessage::Binary(b) => {
                assert_eq!(b.as_ref(), b"input");
            }
            other => panic!("expected binary stdin frame, got {other:?}"),
        }
        match resize_msg {
            WsMessage::Text(t) => {
                let parsed: serde_json::Value =
                    serde_json::from_str(t.as_str()).expect("resize is JSON");
                assert_eq!(parsed["resize"]["rows"], 24);
                assert_eq!(parsed["resize"]["cols"], 80);
            }
            other => panic!("expected text resize frame, got {other:?}"),
        }

        // 4. Close-frame exit code surfaces on the exit future.
        let exit = exit_rx.await.expect("exit_tx delivered");
        let exit = exit.expect("exit future Ok");
        assert_eq!(exit, Some(42));
    }
}
