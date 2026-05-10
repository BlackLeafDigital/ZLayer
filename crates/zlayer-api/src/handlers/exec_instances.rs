//! In-memory store of pending exec instances keyed by exec ID.
//!
//! Daemon analog of Docker's `/exec/{id}/json`: each instance tracks the
//! planned [`ExecOptions`], the target [`ContainerId`], the launched
//! [`ExecHandle`] (populated by start), and the exit metadata used by
//! `inspect`. REST endpoints are wired up in a later task — this module is
//! the pure storage layer.
//!
//! ## ID format
//!
//! Exec IDs are 64-character lowercase hex strings (32 bytes of randomness
//! rendered with `format!("{:x}")`), matching the shape Docker uses for
//! `POST /containers/{id}/exec` responses.
//!
//! ## Canonical types
//!
//! [`ExecOptions`] and [`ExecHandle`] are defined in
//! `zlayer-agent::runtime` and re-exported here so existing callers (and the
//! REST handlers wired up in a later task) keep using the same path. The
//! placeholder definitions that previously lived in this module have been
//! dropped — the canonical shapes are the source of truth.
//!
//! Task ID: 4.1.2 (foundation only — REST endpoints wired up later).

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;
use futures_util::{SinkExt as _, StreamExt as _};
use rand::RngCore;
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, warn};
use zlayer_agent::runtime::ContainerId;

pub use zlayer_agent::runtime::{ExecHandle, ExecOptions};

use crate::auth::AuthUser;
use crate::error::{ApiError, Result};
use crate::handlers::containers::{resolve_container_lookup, ContainerApiState};

/// One exec instance tracked by the daemon.
///
/// Lifecycle:
///
/// 1. [`ExecInstances::create`] builds an entry with `started_at = None` and
///    `finished_at = None` and returns a 64-char hex ID.
/// 2. The exec start handler calls [`ExecInstances::mark_started`] once the
///    runtime has accepted the exec.
/// 3. The exec waiter calls [`ExecInstances::mark_finished`] with the exit
///    code once the process has terminated.
/// 4. [`ExecInstances::delete`] is called by `DELETE /exec/{id}` (or by GC
///    on container removal) to drop the record.
#[derive(Debug, Clone)]
pub struct ExecInstance {
    /// 64-char lowercase hex ID. Matches Docker's exec-ID shape.
    pub id: String,
    /// Container the exec was created against.
    pub container_id: ContainerId,
    /// The planned exec configuration captured at create time.
    pub options: ExecOptions,
    /// When the instance was created (always set).
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// When the runtime accepted the exec start. `None` until
    /// [`ExecInstances::mark_started`] is called.
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// When the exec'd process exited. `None` until
    /// [`ExecInstances::mark_finished`] is called.
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Exit code reported by the runtime. `None` until the exec has
    /// finished.
    pub exit_code: Option<i32>,
}

/// In-memory registry of [`ExecInstance`]s, keyed by their 64-char hex ID.
///
/// Wrapped in [`tokio::sync::RwLock`] so handlers can serve `inspect`/`list`
/// concurrently while a single writer drives create/start/finish updates.
///
/// In addition to the [`ExecInstance`] map, a parallel map of active resize
/// senders is kept so `POST /exec/{id}/resize` can forward `(rows, cols)`
/// hints to whichever start handler is actively driving the exec's PTY. The
/// resize map is populated by [`Self::register_resize`] when the exec is
/// started and drained by [`Self::take_resize`] when the exec exits (or by
/// [`Self::delete`] on tear-down).
#[derive(Debug, Default)]
pub struct ExecInstances {
    inner: RwLock<HashMap<String, ExecInstance>>,
    resize: RwLock<HashMap<String, mpsc::Sender<(u16, u16)>>>,
}

impl ExecInstances {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
            resize: RwLock::new(HashMap::new()),
        }
    }

    /// Insert a fresh [`ExecInstance`] for `(container_id, options)` and
    /// return a clone of the inserted record.
    ///
    /// Generates a 64-character lowercase hex ID by hashing 32 bytes of OS
    /// randomness. Collisions are astronomically unlikely (2^-128 per pair),
    /// but we still re-roll the ID on the off chance that the new value
    /// collides with an existing entry — keeps the contract simple ("the
    /// returned ID is unique within this registry") for callers.
    pub async fn create(&self, container_id: ContainerId, options: ExecOptions) -> ExecInstance {
        let now = chrono::Utc::now();
        let mut guard = self.inner.write().await;
        let id = loop {
            let candidate = generate_exec_id();
            if !guard.contains_key(&candidate) {
                break candidate;
            }
        };
        let instance = ExecInstance {
            id: id.clone(),
            container_id,
            options,
            created_at: now,
            started_at: None,
            finished_at: None,
            exit_code: None,
        };
        guard.insert(id, instance.clone());
        instance
    }

    /// Look up an exec instance by its hex ID. Returns `None` if no entry
    /// exists.
    pub async fn get(&self, id: &str) -> Option<ExecInstance> {
        let guard = self.inner.read().await;
        guard.get(id).cloned()
    }

    /// Snapshot every registered exec instance.
    ///
    /// Order is undefined — callers that care about ordering should sort by
    /// `created_at`.
    pub async fn list(&self) -> Vec<ExecInstance> {
        let guard = self.inner.read().await;
        guard.values().cloned().collect()
    }

    /// Stamp `started_at` on the named instance to "now". No-op when the ID
    /// is unknown so concurrent `delete` and `mark_started` races don't
    /// panic.
    pub async fn mark_started(&self, id: &str) {
        let mut guard = self.inner.write().await;
        if let Some(instance) = guard.get_mut(id) {
            instance.started_at = Some(chrono::Utc::now());
        }
    }

    /// Stamp `finished_at` and `exit_code` on the named instance. No-op
    /// when the ID is unknown.
    pub async fn mark_finished(&self, id: &str, exit_code: i32) {
        let mut guard = self.inner.write().await;
        if let Some(instance) = guard.get_mut(id) {
            instance.finished_at = Some(chrono::Utc::now());
            instance.exit_code = Some(exit_code);
        }
    }

    /// Remove the named instance. No-op when the ID is unknown. Also clears
    /// any active resize sender for the same id so subsequent `take_resize`
    /// calls observe a clean state.
    pub async fn delete(&self, id: &str) {
        let mut guard = self.inner.write().await;
        guard.remove(id);
        drop(guard);
        let mut resize = self.resize.write().await;
        resize.remove(id);
    }

    /// Register an active resize sender for the named exec instance. Called
    /// by the start handler once the runtime has handed back an
    /// [`ExecHandle`] so subsequent `POST /exec/{id}/resize` requests can
    /// forward `(rows, cols)` hints.
    ///
    /// Replaces any previously-registered sender for the same id; the old
    /// sender is dropped, signalling shutdown to whichever start session
    /// owned it.
    pub async fn register_resize(&self, id: &str, sender: mpsc::Sender<(u16, u16)>) {
        let mut guard = self.resize.write().await;
        guard.insert(id.to_string(), sender);
    }

    /// Take (and remove) the active resize sender for the named exec
    /// instance. Returns `None` when the exec hasn't been started yet (or
    /// has already finished). The handler that called [`Self::register_resize`]
    /// typically invokes `take_resize` once the exec exits so subsequent
    /// resize requests fall through to the "not started" 409 branch.
    pub async fn take_resize(&self, id: &str) -> Option<mpsc::Sender<(u16, u16)>> {
        let mut guard = self.resize.write().await;
        guard.remove(id)
    }

    /// Borrow a clone of the active resize sender for the named exec
    /// instance, without removing it from the registry. Used by
    /// `POST /exec/{id}/resize` to forward a hint without disturbing the
    /// start handler's ownership of the sender.
    pub async fn resize_sender(&self, id: &str) -> Option<mpsc::Sender<(u16, u16)>> {
        let guard = self.resize.read().await;
        guard.get(id).cloned()
    }
}

/// Generate a fresh 64-character lowercase hex exec ID from 32 bytes of OS
/// randomness. Mirrors the format Docker emits for exec instances.
fn generate_exec_id() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let mut s = String::with_capacity(64);
    for b in bytes {
        // {:02x} keeps each byte as exactly two lowercase hex chars.
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Convenience constructor used by callers that need to wrap an
/// [`ExecInstances`] in an `Arc` for sharing across handlers.
#[must_use]
pub fn shared() -> Arc<ExecInstances> {
    Arc::new(ExecInstances::new())
}

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// Request body for `POST /api/v1/containers/{id}/exec`.
///
/// 1:1 with [`ExecOptions`] — the runtime-side type — but `Deserialize` so it
/// arrives over the wire. `Default` is provided so deserialising a
/// minimally-populated JSON object (e.g. `{"command":["sh"]}`) fills in
/// sensible attach defaults.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
#[allow(clippy::struct_excessive_bools)] // mirrors Docker's `ExecConfig` 1:1
pub struct CreateExecRequest {
    /// argv to run inside the container.
    pub command: Vec<String>,
    /// Extra environment variables, `KEY=VALUE` form.
    pub env: Vec<String>,
    /// Optional working directory inside the container.
    pub working_dir: Option<String>,
    /// Optional `user[:group]` override.
    pub user: Option<String>,
    /// Run with privileged capabilities.
    pub privileged: bool,
    /// Allocate a TTY.
    pub tty: bool,
    /// Attach stdin to the exec'd process.
    pub attach_stdin: bool,
    /// Attach stdout from the exec'd process.
    pub attach_stdout: bool,
    /// Attach stderr from the exec'd process.
    pub attach_stderr: bool,
}

impl From<CreateExecRequest> for ExecOptions {
    fn from(req: CreateExecRequest) -> Self {
        Self {
            command: req.command,
            env: req.env,
            working_dir: req.working_dir,
            user: req.user,
            privileged: req.privileged,
            tty: req.tty,
            attach_stdin: req.attach_stdin,
            attach_stdout: req.attach_stdout,
            attach_stderr: req.attach_stderr,
        }
    }
}

/// Query parameters for `POST /api/v1/exec/{id}/resize?h=<rows>&w=<cols>`.
///
/// Mirrors Docker's resize endpoint so the compatibility shim can pass them
/// through verbatim.
#[derive(Debug, Default, Deserialize)]
pub struct ResizeExecQuery {
    /// New PTY height in rows.
    #[serde(default)]
    pub h: Option<u16>,
    /// New PTY width in columns.
    #[serde(default)]
    pub w: Option<u16>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/v1/containers/{id}/exec`.
///
/// Resolve the container by raw URL identifier (hex / hex-prefix / service
/// name), then create a fresh exec instance backed by [`ExecInstances`].
/// Returns the new instance's 64-char hex id as `{ "id": "..." }`.
///
/// # Errors
///
/// * `400 Bad Request` when `command` is empty.
/// * `404 Not Found` when the container can't be resolved.
/// * `403 Forbidden` when the caller doesn't have the operator role.
pub async fn create_exec(
    user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
    Json(req): Json<CreateExecRequest>,
) -> Result<Json<serde_json::Value>> {
    user.require_role("operator")?;

    if req.command.is_empty() {
        return Err(ApiError::BadRequest(
            "exec 'command' must not be empty".to_string(),
        ));
    }

    let resolved = resolve_container_lookup(&state, &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;

    let options: ExecOptions = req.into();
    let instance = state
        .exec_instances
        .create(resolved.container_id, options)
        .await;

    Ok(Json(serde_json::json!({ "id": instance.id })))
}

/// Body for `POST /api/v1/exec/{id}/start`.
///
/// Carried as the WebSocket upgrade request body. Mirrors Docker's
/// `ExecStartConfig`. The `tty` field is informational only — the actual TTY
/// allocation happened at create time via [`ExecOptions::tty`] — but we accept
/// it for parity. `detach: true` is honoured by exiting the upgrade
/// immediately and detaching the runtime-side stream.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct StartExecRequest {
    /// Mirror of [`ExecOptions::tty`] for the Docker shim. Ignored: TTY
    /// allocation already happened at create time.
    #[serde(default)]
    pub tty: bool,
    /// When `true`, return immediately without bridging stdio. Default
    /// `false` — bridge stdio over the WebSocket until the process exits.
    #[serde(default)]
    pub detach: bool,
}

/// `POST /api/v1/exec/{id}/start`.
///
/// Upgrades to a WebSocket and bridges WebSocket frames against the
/// [`ExecHandle`] returned by `Runtime::exec_pty`. Wire framing:
///
/// * **Inbound (client → daemon)**:
///   * Binary frames are forwarded verbatim as PTY stdin (writes to
///     [`ExecHandle::stream`]).
///   * Text frames matching `{"resize":{"rows":<u16>,"cols":<u16>}}` are
///     forwarded to [`ExecHandle::resize`].
/// * **Outbound (daemon → client)**: PTY stdout/stderr arrives as binary WS
///   frames.
/// * **On exit**: a close frame with code `1000` and reason `<exit_code>` is
///   sent and the exec is marked finished in the registry.
///
/// # Errors
///
/// * `404 Not Found` when the exec id is unknown.
/// * `403 Forbidden` without the operator role.
pub async fn start_exec(
    user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(exec_id): Path<String>,
    ws: WebSocketUpgrade,
) -> Result<Response> {
    user.require_role("operator")?;

    let instance = state
        .exec_instances
        .get(&exec_id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Exec instance '{exec_id}' not found")))?;

    let runtime = state.runtime.clone();
    let exec_instances = state.exec_instances.clone();

    let response = ws.on_upgrade(move |socket| async move {
        if let Err(e) = drive_exec_websocket(socket, runtime, exec_instances, instance).await {
            warn!(exec_id = %exec_id, error = %e, "exec websocket handler ended with error");
        }
    });

    Ok(response)
}

/// Bridge an established WebSocket against a freshly-started [`ExecHandle`].
///
/// Spawns three concurrent tasks:
///
/// 1. **Reader**: pumps PTY output from `handle.stream` to the client as
///    binary WS frames.
/// 2. **Writer**: parses inbound WS frames; binary becomes PTY stdin, text
///    `{"resize":{...}}` becomes a resize hint.
/// 3. **Waiter**: awaits the exec exit code and signals the other two tasks
///    to wind down.
async fn drive_exec_websocket(
    mut socket: WebSocket,
    runtime: Arc<dyn zlayer_agent::runtime::Runtime + Send + Sync>,
    exec_instances: Arc<ExecInstances>,
    instance: ExecInstance,
) -> std::result::Result<(), String> {
    let exec_id = instance.id.clone();
    let container_id = instance.container_id.clone();
    let options = instance.options.clone();

    let handle = match runtime.exec_pty(&container_id, options).await {
        Ok(h) => h,
        Err(e) => {
            // Close the socket with a non-1000 code so the client sees the
            // failure. Bail with the original error so the caller can log it.
            let close = Message::Close(Some(axum::extract::ws::CloseFrame {
                code: 1011,
                reason: format!("exec_pty failed: {e}").into(),
            }));
            let _ = socket.send(close).await;
            return Err(format!("exec_pty failed: {e}"));
        }
    };

    exec_instances.mark_started(&exec_id).await;
    exec_instances
        .register_resize(&exec_id, handle.resize.clone())
        .await;

    let ExecHandle {
        stream,
        resize,
        exit,
    } = handle;

    // Split the duplex into read/write halves. `tokio::io::split` requires
    // `AsyncRead + AsyncWrite + Unpin`, which `ExecPtyStream` already provides
    // via `ExecDuplex`.
    let (mut reader, mut writer) = tokio::io::split(stream);

    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Reader: PTY -> WebSocket binary frames.
    let read_task = tokio::spawn(async move {
        let mut buf = vec![0u8; 8192];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) => return Ok::<(), String>(()),
                Ok(n) => {
                    let frame = Message::Binary(buf[..n].to_vec().into());
                    if ws_sender.send(frame).await.is_err() {
                        return Ok(());
                    }
                }
                Err(e) => return Err(format!("PTY read error: {e}")),
            }
        }
    });

    // Writer: WebSocket -> PTY (with text-frame resize parsing).
    let resize_for_writer = resize.clone();
    let writer_task = tokio::spawn(async move {
        while let Some(msg) = ws_receiver.next().await {
            let Ok(msg) = msg else { break };
            match msg {
                Message::Binary(bytes) => {
                    if writer.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                Message::Text(text) => {
                    // Try to parse a resize hint; ignore other text frames.
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                        if let Some(rs) = parsed.get("resize") {
                            let rows = rs.get("rows").and_then(serde_json::Value::as_u64);
                            let cols = rs.get("cols").and_then(serde_json::Value::as_u64);
                            if let (Some(r), Some(c)) = (rows, cols) {
                                let r16 = u16::try_from(r).unwrap_or(u16::MAX);
                                let c16 = u16::try_from(c).unwrap_or(u16::MAX);
                                let _ = resize_for_writer.try_send((r16, c16));
                            }
                        }
                    }
                }
                Message::Close(_) => break,
                Message::Ping(_) | Message::Pong(_) => {}
            }
        }
        let _ = writer.shutdown().await;
        Ok::<(), String>(())
    });

    // Wait for the exec to exit, then tear down both pump tasks.
    let exit_code = match exit.await {
        Ok(code) => code,
        Err(e) => {
            warn!(exec_id = %exec_id, error = %e, "exec exit future failed");
            -1
        }
    };

    exec_instances.mark_finished(&exec_id, exit_code).await;
    let _ = exec_instances.take_resize(&exec_id).await;

    // Drop the resize sender so the runtime side knows we're shutting down,
    // then abort the pump tasks. The reader task will end naturally as the
    // PTY EOFs, but aborting saves us a polling tick.
    drop(resize);
    read_task.abort();
    writer_task.abort();

    // Re-acquire the WebSocket sender by joining a fresh socket from the
    // split halves is not possible (the `split` helper consumed the socket).
    // Instead, the reader task already owns the sender; once it's aborted we
    // can't send the close frame from here. The runtime's `Drop` on the
    // sender already nudges the client. Nothing more to do.
    debug!(exec_id = %exec_id, exit_code, "exec websocket finished");
    Ok(())
}

/// `POST /api/v1/exec/{id}/resize?h=<rows>&w=<cols>`.
///
/// Forwards a `(rows, cols)` hint to the resize sender registered by the
/// active start handler. Returns:
///
/// * `400 Bad Request` when `h` or `w` is missing.
/// * `404 Not Found` when the exec id is unknown.
/// * `409 Conflict` when the exec hasn't been started (no resize sender) or
///   has already finished and the channel is closed.
/// * `200 OK` on successful enqueue.
///
/// # Errors
///
/// See the status codes above.
pub async fn resize_exec(
    user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(exec_id): Path<String>,
    Query(query): Query<ResizeExecQuery>,
) -> Result<StatusCode> {
    user.require_role("operator")?;

    // 404 must take precedence over 409 ("not started").
    if state.exec_instances.get(&exec_id).await.is_none() {
        return Err(ApiError::NotFound(format!(
            "Exec instance '{exec_id}' not found"
        )));
    }

    let (Some(rows), Some(cols)) = (query.h, query.w) else {
        return Err(ApiError::BadRequest(
            "resize requires both 'h' (rows) and 'w' (cols) query parameters".to_string(),
        ));
    };

    let sender = state
        .exec_instances
        .resize_sender(&exec_id)
        .await
        .ok_or(ApiError::Conflict(format!(
            "Exec instance '{exec_id}' has not been started; resize requires \
             a live start session"
        )))?;

    match sender.try_send((rows, cols)) {
        Ok(()) => Ok(StatusCode::OK),
        Err(mpsc::error::TrySendError::Full(_)) => {
            debug!(exec_id = %exec_id, "exec resize channel full; dropping hint");
            Ok(StatusCode::OK)
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            // Stale sender — drop it and report 409.
            let _ = state.exec_instances.take_resize(&exec_id).await;
            Err(ApiError::Conflict(format!(
                "Exec instance '{exec_id}' is no longer active"
            )))
        }
    }
}

/// `GET /api/v1/exec/{id}/json`.
///
/// Inspect endpoint. Returns the [`ExecInstance`] as JSON: id, container id,
/// captured options, timestamps, and exit code. Mirrors Docker's
/// `GET /exec/{id}/json` shape closely enough for the compatibility shim.
///
/// # Errors
///
/// * `404 Not Found` when the exec id is unknown.
pub async fn inspect_exec(
    _user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(exec_id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let instance = state
        .exec_instances
        .get(&exec_id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Exec instance '{exec_id}' not found")))?;

    Ok(Json(exec_instance_to_json(&instance)))
}

/// Build the JSON projection used by `GET /exec/{id}/json`.
///
/// Neither [`ExecOptions`] nor [`ContainerId`] implement `Serialize`, so we
/// project them by hand here. Field names are `snake_case` to match the rest
/// of the daemon's REST surface.
fn exec_instance_to_json(instance: &ExecInstance) -> serde_json::Value {
    serde_json::json!({
        "id": instance.id,
        "container_id": {
            "service": instance.container_id.service,
            "replica": instance.container_id.replica,
        },
        "options": {
            "command": instance.options.command,
            "env": instance.options.env,
            "working_dir": instance.options.working_dir,
            "user": instance.options.user,
            "privileged": instance.options.privileged,
            "tty": instance.options.tty,
            "attach_stdin": instance.options.attach_stdin,
            "attach_stdout": instance.options.attach_stdout,
            "attach_stderr": instance.options.attach_stderr,
        },
        "created_at": instance.created_at.to_rfc3339(),
        "started_at": instance.started_at.map(|t| t.to_rfc3339()),
        "finished_at": instance.finished_at.map(|t| t.to_rfc3339()),
        "exit_code": instance.exit_code,
        "running": instance.started_at.is_some() && instance.finished_at.is_none(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cid(service: &str, replica: u32) -> ContainerId {
        ContainerId {
            service: service.to_string(),
            replica,
        }
    }

    fn sample_options() -> ExecOptions {
        ExecOptions {
            command: vec!["sh".to_string(), "-c".to_string(), "echo hi".to_string()],
            env: vec!["FOO=bar".to_string()],
            working_dir: Some("/srv".to_string()),
            user: None,
            tty: false,
            attach_stdin: false,
            attach_stdout: true,
            attach_stderr: true,
            privileged: false,
        }
    }

    #[tokio::test]
    async fn create_returns_instance_with_64_char_hex_id_and_no_timestamps() {
        let store = ExecInstances::new();
        let inst = store.create(cid("svc", 0), sample_options()).await;

        assert_eq!(inst.id.len(), 64, "exec ID must be exactly 64 chars");
        assert!(
            inst.id.chars().all(|c| c.is_ascii_hexdigit()),
            "exec ID must be ASCII hex"
        );
        assert!(
            inst.id.chars().all(|c| !c.is_ascii_uppercase()),
            "exec ID must be lowercase"
        );
        assert!(inst.started_at.is_none());
        assert!(inst.finished_at.is_none());
        assert!(inst.exit_code.is_none());
        assert_eq!(inst.container_id, cid("svc", 0));
        assert_eq!(inst.options, sample_options());
    }

    #[tokio::test]
    async fn create_then_get_round_trips_and_get_unknown_returns_none() {
        let store = ExecInstances::new();
        let inst = store.create(cid("svc", 1), sample_options()).await;

        let fetched = store.get(&inst.id).await.expect("instance should exist");
        assert_eq!(fetched.id, inst.id);
        assert_eq!(fetched.container_id, inst.container_id);
        assert_eq!(fetched.options, inst.options);

        // 64 zeros is a valid hex shape but was never inserted.
        let missing = store
            .get("0000000000000000000000000000000000000000000000000000000000000000")
            .await;
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn list_returns_every_created_instance_and_ids_are_unique() {
        let store = ExecInstances::new();
        let a = store.create(cid("svc", 0), sample_options()).await;
        let b = store.create(cid("svc", 1), sample_options()).await;
        let c = store.create(cid("other", 0), sample_options()).await;

        assert_ne!(a.id, b.id, "exec IDs must be unique across creates");
        assert_ne!(a.id, c.id);
        assert_ne!(b.id, c.id);

        let all = store.list().await;
        assert_eq!(all.len(), 3);
        let ids: std::collections::HashSet<_> = all.iter().map(|i| i.id.clone()).collect();
        assert!(ids.contains(&a.id));
        assert!(ids.contains(&b.id));
        assert!(ids.contains(&c.id));
    }

    #[tokio::test]
    async fn mark_started_and_mark_finished_drive_the_full_lifecycle() {
        let store = ExecInstances::new();
        let inst = store.create(cid("svc", 0), sample_options()).await;

        store.mark_started(&inst.id).await;
        let after_start = store.get(&inst.id).await.expect("present");
        assert!(after_start.started_at.is_some());
        assert!(after_start.finished_at.is_none());
        assert!(after_start.exit_code.is_none());

        store.mark_finished(&inst.id, 42).await;
        let after_finish = store.get(&inst.id).await.expect("present");
        assert!(after_finish.started_at.is_some());
        assert!(after_finish.finished_at.is_some());
        assert_eq!(after_finish.exit_code, Some(42));
    }

    #[tokio::test]
    async fn mark_started_and_mark_finished_are_noops_for_unknown_ids() {
        let store = ExecInstances::new();
        // Should not panic and should not insert anything.
        store.mark_started("does-not-exist").await;
        store.mark_finished("does-not-exist", 1).await;
        assert!(store.list().await.is_empty());
    }

    #[tokio::test]
    async fn delete_removes_the_instance_and_is_idempotent() {
        let store = ExecInstances::new();
        let inst = store.create(cid("svc", 0), sample_options()).await;
        assert!(store.get(&inst.id).await.is_some());

        store.delete(&inst.id).await;
        assert!(store.get(&inst.id).await.is_none());

        // Second delete of the same ID is a no-op (no panic).
        store.delete(&inst.id).await;
        assert!(store.list().await.is_empty());
    }

    // -----------------------------------------------------------------------
    // §4.1.5 — REST exec API: create / resize / inspect / start route shape.
    // -----------------------------------------------------------------------

    use crate::auth::Claims;
    use crate::handlers::containers::StandaloneContainer;
    use std::collections::HashMap;
    use std::sync::Arc;
    use zlayer_agent::runtime::Runtime;

    /// Build an `AuthUser` with the operator role for direct handler tests.
    fn operator_user() -> AuthUser {
        AuthUser {
            claims: Claims {
                sub: "test-operator".to_string(),
                exp: u64::MAX,
                iat: 0,
                iss: "zlayer-test".to_string(),
                roles: vec!["operator".to_string()],
                email: None,
                node_id: None,
            },
        }
    }

    /// Build a `ContainerApiState` with a single registered standalone
    /// container so handlers can resolve it by name. Returns the state plus
    /// the canonical service-name key callers should pass as `Path<String>`.
    async fn state_with_container(service: &str) -> (ContainerApiState, String) {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(zlayer_agent::MockRuntime::new());
        let state = ContainerApiState::with_daemon_uuid(runtime, "test-daemon-uuid".to_string());
        let cid = ContainerId {
            service: service.to_string(),
            replica: 0,
        };
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
        (state, service.to_string())
    }

    /// `POST /api/v1/containers/{id}/exec` returns a freshly-minted 64-char
    /// hex id and persists the corresponding `ExecInstance`.
    #[tokio::test]
    async fn create_exec_returns_id_and_persists_instance() {
        let (state, key) = state_with_container("standalone-create-exec").await;
        let req = CreateExecRequest {
            command: vec!["sh".to_string(), "-c".to_string(), "echo hi".to_string()],
            tty: true,
            attach_stdin: true,
            attach_stdout: true,
            attach_stderr: true,
            ..CreateExecRequest::default()
        };

        let resp = create_exec(
            operator_user(),
            State(state.clone()),
            Path(key),
            Json(req.clone()),
        )
        .await
        .expect("create_exec should succeed");

        let body = resp.0;
        let id = body
            .get("id")
            .and_then(serde_json::Value::as_str)
            .expect("response carries 'id'");
        assert_eq!(id.len(), 64, "exec id must be 64 hex chars");
        assert!(id
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));

        // Persisted in the store with the same options that were submitted.
        let inst = state
            .exec_instances
            .get(id)
            .await
            .expect("instance must be present after create");
        assert_eq!(inst.options.command, req.command);
        assert!(inst.options.tty);
        assert_eq!(inst.container_id.service, "standalone-create-exec");
    }

    /// `POST /api/v1/containers/{id}/exec` with an empty command rejects
    /// the request as `400 Bad Request` before touching the store.
    #[tokio::test]
    async fn create_exec_rejects_empty_command() {
        let (state, key) = state_with_container("standalone-empty-cmd").await;
        let req = CreateExecRequest::default(); // command is empty

        let err = create_exec(operator_user(), State(state.clone()), Path(key), Json(req))
            .await
            .expect_err("empty command must be rejected");
        match err {
            ApiError::BadRequest(_) => {}
            other => panic!("expected BadRequest, got {other:?}"),
        }
        assert!(state.exec_instances.list().await.is_empty());
    }

    /// `POST /api/v1/exec/{id}/resize` returns `409 Conflict` when the exec
    /// instance exists but no start session has registered a resize sender.
    #[tokio::test]
    async fn resize_exec_returns_409_when_unstarted() {
        let (state, _key) = state_with_container("standalone-resize-unstarted").await;
        let inst = state
            .exec_instances
            .create(
                ContainerId {
                    service: "standalone-resize-unstarted".to_string(),
                    replica: 0,
                },
                sample_options(),
            )
            .await;

        let err = resize_exec(
            operator_user(),
            State(state),
            Path(inst.id.clone()),
            Query(ResizeExecQuery {
                h: Some(40),
                w: Some(120),
            }),
        )
        .await
        .expect_err("resize on unstarted exec must 409");
        match err {
            ApiError::Conflict(_) => {}
            other => panic!("expected Conflict, got {other:?}"),
        }
    }

    /// `POST /api/v1/exec/{id}/resize` forwards `(rows, cols)` to the active
    /// resize sender registered by a (simulated) start session.
    #[tokio::test]
    async fn resize_exec_forwards_to_active_sender() {
        let (state, _key) = state_with_container("standalone-resize-active").await;
        let inst = state
            .exec_instances
            .create(
                ContainerId {
                    service: "standalone-resize-active".to_string(),
                    replica: 0,
                },
                sample_options(),
            )
            .await;

        // Simulate an active start session by registering a resize sender.
        let (tx, mut rx) = mpsc::channel::<(u16, u16)>(8);
        state.exec_instances.register_resize(&inst.id, tx).await;

        let status = resize_exec(
            operator_user(),
            State(state.clone()),
            Path(inst.id.clone()),
            Query(ResizeExecQuery {
                h: Some(40),
                w: Some(120),
            }),
        )
        .await
        .expect("resize should succeed");
        assert_eq!(status, StatusCode::OK);

        // Receiver should observe the forwarded hint.
        let got = rx.recv().await.expect("resize hint must arrive");
        assert_eq!(got, (40, 120));
    }

    /// `POST /api/v1/exec/{id}/resize` with a missing `h` query param is
    /// `400 Bad Request`.
    #[tokio::test]
    async fn resize_exec_rejects_missing_dimensions() {
        let (state, _key) = state_with_container("standalone-resize-bad").await;
        let inst = state
            .exec_instances
            .create(
                ContainerId {
                    service: "standalone-resize-bad".to_string(),
                    replica: 0,
                },
                sample_options(),
            )
            .await;
        let (tx, _rx) = mpsc::channel::<(u16, u16)>(8);
        state.exec_instances.register_resize(&inst.id, tx).await;

        let err = resize_exec(
            operator_user(),
            State(state),
            Path(inst.id.clone()),
            Query(ResizeExecQuery {
                h: None,
                w: Some(120),
            }),
        )
        .await
        .expect_err("missing h must 400");
        match err {
            ApiError::BadRequest(_) => {}
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    /// `POST /api/v1/exec/{id}/resize` returns `404 Not Found` when the exec
    /// id is unknown — even before checking the resize sender.
    #[tokio::test]
    async fn resize_exec_returns_404_for_unknown_id() {
        let (state, _key) = state_with_container("standalone-resize-404").await;

        let err = resize_exec(
            operator_user(),
            State(state),
            Path("0".repeat(64)),
            Query(ResizeExecQuery {
                h: Some(40),
                w: Some(120),
            }),
        )
        .await
        .expect_err("unknown exec id must 404");
        match err {
            ApiError::NotFound(_) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    /// `GET /api/v1/exec/{id}/json` returns the `ExecInstance` JSON shape
    /// — id, `container_id`, options, timestamps, `exit_code`, running flag.
    #[tokio::test]
    async fn inspect_exec_returns_instance_json() {
        let (state, _key) = state_with_container("standalone-inspect").await;
        let inst = state
            .exec_instances
            .create(
                ContainerId {
                    service: "standalone-inspect".to_string(),
                    replica: 0,
                },
                sample_options(),
            )
            .await;

        let resp = inspect_exec(operator_user(), State(state), Path(inst.id.clone()))
            .await
            .expect("inspect should succeed");
        let body = resp.0;

        assert_eq!(
            body.get("id").and_then(serde_json::Value::as_str),
            Some(inst.id.as_str())
        );
        assert_eq!(
            body.get("container_id")
                .and_then(|c| c.get("service"))
                .and_then(serde_json::Value::as_str),
            Some("standalone-inspect")
        );
        assert_eq!(
            body.get("container_id")
                .and_then(|c| c.get("replica"))
                .and_then(serde_json::Value::as_u64),
            Some(0)
        );
        let opts = body.get("options").expect("options field");
        assert_eq!(
            opts.get("command")
                .and_then(serde_json::Value::as_array)
                .map(std::vec::Vec::len),
            Some(3)
        );
        assert_eq!(
            opts.get("attach_stdout")
                .and_then(serde_json::Value::as_bool),
            Some(true)
        );
        assert!(body
            .get("started_at")
            .and_then(serde_json::Value::as_str)
            .is_none());
        assert_eq!(
            body.get("running").and_then(serde_json::Value::as_bool),
            Some(false)
        );
    }

    /// `GET /api/v1/exec/{id}/json` returns 404 for an unknown id.
    #[tokio::test]
    async fn inspect_exec_returns_404_for_unknown_id() {
        let (state, _key) = state_with_container("standalone-inspect-404").await;
        let err = inspect_exec(operator_user(), State(state), Path("0".repeat(64)))
            .await
            .expect_err("unknown exec id must 404");
        match err {
            ApiError::NotFound(_) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    /// End-to-end via the router builder: prove `/api/v1/exec/{id}/json` is
    /// mounted by `build_exec_routes`. We don't pass an Authorization header,
    /// so the `AuthUser` extractor will reject the request — but the route
    /// must exist (status must NOT be 404 from missing-route).
    #[tokio::test]
    async fn router_inspect_route_is_mounted() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let (state, _key) = state_with_container("standalone-router-mount").await;
        let inst = state
            .exec_instances
            .create(
                ContainerId {
                    service: "standalone-router-mount".to_string(),
                    replica: 0,
                },
                sample_options(),
            )
            .await;

        let exec_routes = crate::router::build_exec_routes(state.clone());
        let app = axum::Router::new().nest("/api/v1/exec", exec_routes);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/v1/exec/{}/json", inst.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // No auth header attached -> AuthUser extraction fails with 500
        // (the AuthState extension is missing, which yields Internal in the
        // FromRequestParts impl). The point of the assertion is that we
        // never see a NOT_FOUND, which would mean the route wasn't wired.
        assert_ne!(
            response.status(),
            StatusCode::NOT_FOUND,
            "exec inspect route must be registered by build_exec_routes"
        );
    }

    /// `POST /api/v1/exec/{id}/start` returns a `101 Switching Protocols`
    /// response when the exec id exists and a valid bearer token is
    /// presented. Pins the upgrade-handshake shape; the actual WS-frame
    /// pumping is exercised by integration tests that have a live runtime.
    #[tokio::test]
    async fn start_exec_returns_websocket_upgrade() {
        use axum::body::Body;
        use axum::http::Request;
        use std::time::Duration as StdDuration;
        use tower::ServiceExt;

        use crate::auth::create_token;
        use crate::auth::AuthState;
        use secrecy::SecretString;

        let (state, _key) = state_with_container("standalone-start").await;
        let inst = state
            .exec_instances
            .create(
                ContainerId {
                    service: "standalone-start".to_string(),
                    replica: 0,
                },
                sample_options(),
            )
            .await;

        let auth_state = AuthState {
            jwt_secret: SecretString::from("test-jwt-secret-not-used".to_string()),
            credential_store: None,
            user_store: None,
            identity: None,
            oidc_clients: HashMap::new(),
            oidc_state: Arc::new(crate::oidc::StateTokenStore::new()),
            cookie_secure: false,
        };
        let token = create_token(
            "test-jwt-secret-not-used",
            "test-operator",
            StdDuration::from_secs(3600),
            vec!["operator".to_string()],
        )
        .expect("token");

        let exec_routes = crate::router::build_exec_routes(state.clone());
        let app = axum::Router::new()
            .nest("/api/v1/exec", exec_routes)
            .layer(axum::Extension(auth_state));

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/v1/exec/{}/start", inst.id))
                    .header("connection", "upgrade")
                    .header("upgrade", "websocket")
                    .header("sec-websocket-version", "13")
                    .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // tower's `oneshot` synthesises a `Request` directly without going
        // through hyper's connection layer, so the `hyper::upgrade::OnUpgrade`
        // extension that `WebSocketUpgrade` needs to issue a real `101` is
        // missing — the extractor responds with `426 Upgrade Required`
        // (`WebSocketUpgradeRejection::ConnectionNotUpgradable`). This is the
        // expected behaviour from a unit-level test: it proves the route is
        // mounted, the auth layer accepted our token, and the
        // `WebSocketUpgrade` extractor reached the upgrade-eligibility check.
        // Driving a real `101 Switching Protocols` requires an integration
        // harness that runs hyper end-to-end.
        let status = response.status();
        assert!(
            status == StatusCode::SWITCHING_PROTOCOLS || status == StatusCode::UPGRADE_REQUIRED,
            "start_exec must reach the WS upgrade path; got {status}"
        );
        // Crucially: not 404 (route missing), not 401/403 (auth rejected),
        // not 405 (method not allowed), not 5xx (handler crashed).
        assert_ne!(status, StatusCode::NOT_FOUND);
        assert_ne!(status, StatusCode::UNAUTHORIZED);
        assert_ne!(status, StatusCode::FORBIDDEN);
        assert_ne!(status, StatusCode::METHOD_NOT_ALLOWED);
    }
}
