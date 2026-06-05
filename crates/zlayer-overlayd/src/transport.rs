//! Length-prefixed JSON framing over a Unix domain socket (Unix) or a named
//! pipe (Windows), plus the server accept loop and client connector.
//!
//! Wire frame: a `u32` little-endian byte length followed by that many bytes
//! of JSON ([`zlayer_types::overlayd::OverlaydFrame`]). One connection
//! multiplexes request/response and server→client event push — both are just
//! frames in either direction.
//!
//! The framing is deliberately tiny and dependency-free (no `tokio-util`):
//! [`FramedConn`] wraps any `AsyncRead + AsyncWrite` (a `UnixStream`, a named
//! pipe instance, or an in-memory `tokio::io::duplex` for tests).

use std::path::Path;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zlayer_types::overlayd::OverlaydFrame;

use crate::error::{OverlaydError, Result, MAX_FRAME_BYTES};

/// A framed connection over an async byte stream.
pub struct FramedConn<S> {
    stream: S,
}

impl<S: AsyncReadExt + AsyncWriteExt + Unpin> FramedConn<S> {
    /// Wrap a byte stream.
    pub fn new(stream: S) -> Self {
        Self { stream }
    }

    /// Consume the wrapper and return the inner stream.
    pub fn into_inner(self) -> S {
        self.stream
    }

    /// Send one frame: `u32` LE length prefix + JSON body, then flush.
    ///
    /// # Errors
    /// [`OverlaydError::FrameTooLarge`] if the serialized body exceeds
    /// [`MAX_FRAME_BYTES`]; [`OverlaydError::Codec`]/[`OverlaydError::Io`] on
    /// serialization / write failure.
    pub async fn send(&mut self, frame: &OverlaydFrame) -> Result<()> {
        let body = serde_json::to_vec(frame)?;
        if body.len() > MAX_FRAME_BYTES {
            return Err(OverlaydError::FrameTooLarge(body.len()));
        }
        let len =
            u32::try_from(body.len()).map_err(|_| OverlaydError::FrameTooLarge(body.len()))?;
        self.stream.write_all(&len.to_le_bytes()).await?;
        self.stream.write_all(&body).await?;
        self.stream.flush().await?;
        Ok(())
    }

    /// Receive one frame. Returns [`OverlaydError::Closed`] on a clean EOF at a
    /// frame boundary (peer hung up).
    ///
    /// # Errors
    /// [`OverlaydError::FrameTooLarge`] if the advertised length exceeds
    /// [`MAX_FRAME_BYTES`]; [`OverlaydError::Codec`]/[`OverlaydError::Io`] on
    /// short read / bad JSON.
    pub async fn recv(&mut self) -> Result<OverlaydFrame> {
        let mut len_buf = [0u8; 4];
        match self.stream.read_exact(&mut len_buf).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(OverlaydError::Closed);
            }
            Err(e) => return Err(e.into()),
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        if len > MAX_FRAME_BYTES {
            return Err(OverlaydError::FrameTooLarge(len));
        }
        let mut body = vec![0u8; len];
        self.stream.read_exact(&mut body).await?;
        Ok(serde_json::from_slice(&body)?)
    }
}

// ---------------------------------------------------------------------------
// Server accept loop
// ---------------------------------------------------------------------------

/// Listen on `endpoint` and invoke `handler` once per accepted connection,
/// each on its own task. `endpoint` is a filesystem path for the Unix socket
/// or the pipe name (e.g. `\\.\pipe\zlayer-overlayd`) on Windows.
///
/// The handler owns the [`FramedConn`] for the connection's lifetime — it
/// should loop `recv()` → dispatch → `send()` and may also push event frames.
///
/// # Errors
/// Propagates bind/accept errors. Per-connection handler errors are logged,
/// not propagated (one bad client must not take down the listener).
#[cfg(unix)]
pub async fn serve<F, Fut>(endpoint: &Path, handler: F) -> Result<()>
where
    F: Fn(FramedConn<tokio::net::UnixStream>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    use std::sync::Arc;

    if tokio::fs::try_exists(endpoint).await.unwrap_or(false) {
        // Remove a stale socket from a prior run so bind() succeeds.
        let _ = tokio::fs::remove_file(endpoint).await;
    }
    if let Some(parent) = endpoint.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let listener = tokio::net::UnixListener::bind(endpoint)?;

    // overlayd runs as ROOT (it owns the system utun adapter), so a bare bind
    // leaves the IPC socket root-owned and not connectable by the per-user
    // daemon — which then fails `setup_global_overlay` with "Permission denied"
    // and silently loses cross-node networking. Mirror the main daemon's API
    // socket policy (see `zlayer-api`'s `server.rs`): 0o660 + chown to the
    // shared `zlayer` group so group members (the user daemon) can connect.
    #[allow(unsafe_code)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        use std::os::unix::fs::PermissionsExt as _;
        if let Err(e) = std::fs::set_permissions(endpoint, std::fs::Permissions::from_mode(0o660)) {
            tracing::debug!(error = %e, socket = %endpoint.display(), "failed to set overlayd socket perms 0o660");
        }
        // chown the socket to the shared `zlayer` group via libc (`nix` is not a
        // macOS dependency of this crate). `getgrnam` is fine for a one-shot
        // startup lookup. `uid = u32::MAX` leaves the owner unchanged.
        if let (Ok(path_c), Ok(gname)) = (
            std::ffi::CString::new(endpoint.as_os_str().as_bytes()),
            std::ffi::CString::new("zlayer"),
        ) {
            // SAFETY: `gname`/`path_c` are valid NUL-terminated C strings that
            // outlive the calls; we only read scalar fields from the returned
            // `group` pointer when non-null.
            unsafe {
                let grp = libc::getgrnam(gname.as_ptr());
                if grp.is_null() {
                    tracing::debug!(socket = %endpoint.display(), "group 'zlayer' not present; skipping overlayd socket chown");
                } else {
                    let gid = (*grp).gr_gid;
                    if libc::chown(path_c.as_ptr(), u32::MAX, gid) == 0 {
                        tracing::info!(socket = %endpoint.display(), "overlayd socket chowned to <owner>:zlayer 0o660");
                    } else {
                        tracing::debug!(error = %std::io::Error::last_os_error(), socket = %endpoint.display(), "failed to chown overlayd socket to zlayer group");
                    }
                }
            }
        }
    }

    tracing::info!(endpoint = %endpoint.display(), "overlayd IPC listening (unix socket)");
    let handler = Arc::new(handler);
    loop {
        let (stream, _addr) = listener.accept().await?;
        let handler = Arc::clone(&handler);
        tokio::spawn(async move {
            handler(FramedConn::new(stream)).await;
        });
    }
}

/// Windows named-pipe accept loop. Named pipes are single-instance servers, so
/// each iteration creates a fresh instance, waits for one client, hands it to
/// the handler on a task, then loops to create the next instance.
///
/// # Errors
/// Propagates pipe create/connect errors.
#[cfg(windows)]
pub async fn serve<F, Fut>(endpoint: &Path, handler: F) -> Result<()>
where
    F: Fn(FramedConn<tokio::net::windows::named_pipe::NamedPipeServer>) -> Fut
        + Send
        + Sync
        + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    use std::sync::Arc;
    use tokio::net::windows::named_pipe::ServerOptions;

    let pipe_name = endpoint
        .to_str()
        .ok_or_else(|| OverlaydError::Other("named-pipe path is not valid UTF-8".to_string()))?
        .to_string();
    tracing::info!(endpoint = %pipe_name, "overlayd IPC listening (named pipe)");
    let handler = Arc::new(handler);
    loop {
        let server = ServerOptions::new()
            .first_pipe_instance(false)
            .create(&pipe_name)?;
        server.connect().await?;
        let handler = Arc::clone(&handler);
        tokio::spawn(async move {
            handler(FramedConn::new(server)).await;
        });
    }
}

// ---------------------------------------------------------------------------
// Client connect
// ---------------------------------------------------------------------------

/// A connected client stream (platform-specific concrete type) wrapped in a
/// [`FramedConn`]. Used by [`crate::client`].
#[cfg(unix)]
pub type ClientConn = FramedConn<tokio::net::UnixStream>;
/// A connected client stream (platform-specific concrete type) wrapped in a
/// [`FramedConn`]. Used by [`crate::client`].
#[cfg(windows)]
pub type ClientConn = FramedConn<tokio::net::windows::named_pipe::NamedPipeClient>;

/// Connect once to the overlayd endpoint.
///
/// # Errors
/// Propagates the underlying connect error.
#[cfg(unix)]
pub async fn connect(endpoint: &Path) -> Result<ClientConn> {
    let stream = tokio::net::UnixStream::connect(endpoint).await?;
    Ok(FramedConn::new(stream))
}

/// Connect once to the overlayd endpoint (Windows named pipe).
///
/// # Errors
/// Propagates the underlying open error.
#[cfg(windows)]
// Must stay `async` to match the unix `connect` signature (callers `.await` it);
// the named-pipe open is synchronous, so there's nothing to await here.
#[allow(clippy::unused_async)]
pub async fn connect(endpoint: &Path) -> Result<ClientConn> {
    use tokio::net::windows::named_pipe::ClientOptions;
    let pipe_name = endpoint
        .to_str()
        .ok_or_else(|| OverlaydError::Other("named-pipe path is not valid UTF-8".to_string()))?;
    let client = ClientOptions::new().open(pipe_name)?;
    Ok(FramedConn::new(client))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zlayer_types::overlayd::{OverlaydRequest, OverlaydResponse};

    #[tokio::test]
    async fn frames_round_trip_over_duplex() {
        // An in-memory bidirectional pipe stands in for a socket.
        let (a, b) = tokio::io::duplex(64 * 1024);
        let mut client = FramedConn::new(a);
        let mut server = FramedConn::new(b);

        let req = OverlaydFrame::Request {
            id: 7,
            request: OverlaydRequest::Status,
        };
        client.send(&req).await.unwrap();
        let got = server.recv().await.unwrap();
        assert_eq!(got, req);

        let resp = OverlaydFrame::Response {
            id: 7,
            response: OverlaydResponse::Ok,
        };
        server.send(&resp).await.unwrap();
        assert_eq!(client.recv().await.unwrap(), resp);
    }

    #[tokio::test]
    async fn clean_eof_maps_to_closed() {
        let (a, b) = tokio::io::duplex(1024);
        drop(a); // peer hangs up
        let mut server = FramedConn::new(b);
        assert!(matches!(server.recv().await, Err(OverlaydError::Closed)));
    }
}
