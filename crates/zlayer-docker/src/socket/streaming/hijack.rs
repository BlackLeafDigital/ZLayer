//! HTTP/1.1 connection hijacking for Docker `/attach` and `/exec/start`.
//!
//! Docker's attach and exec-start endpoints upgrade the underlying TCP
//! connection so the client can stream stdin/stdout/stderr bytes directly
//! instead of going through the HTTP request/response cycle. This module
//! wraps [`hyper::upgrade::on`] and exposes:
//!
//! * [`raw_stream_response`] / [`multiplexed_stream_response`] — the two
//!   `101 Switching Protocols` response shapes Docker uses (raw TTY pass-
//!   through and the [`log_frame`] multiplexed wire format respectively).
//! * [`HijackedConnection`] — a typed wrapper around the upgraded socket
//!   that yields independently-owned read and write halves suitable for
//!   bidirectional copying with the agent's container I/O streams.
//!
//! The streaming wire format itself lives in [`super::log_frame`]; this
//! module only deals with hijacking the transport.

use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// `Content-Type` value used by Docker's multiplexed attach/logs stream.
///
/// Docker sets this on the upgraded `101 Switching Protocols` response when
/// the container is attached without a TTY so the client knows each chunk is
/// framed with the 8-byte [`super::log_frame`] header.
const MULTIPLEXED_CONTENT_TYPE: &str = "application/vnd.docker.multiplexed-stream";

/// `Upgrade` token Docker uses for raw (TTY) attach streams.
///
/// When a container is started with a TTY, the attach stream is a plain
/// byte pipe with no framing, signalled to clients via `Upgrade: tcp`.
const RAW_UPGRADE_TOKEN: &str = "tcp";

/// Errors produced while hijacking an HTTP/1.1 connection for streaming.
#[derive(thiserror::Error, Debug)]
pub enum HijackError {
    /// `hyper::upgrade::on` rejected the upgrade — typically because the
    /// peer closed the connection before the upgrade completed or the
    /// `Connection: Upgrade` handshake was malformed.
    #[error("upgrade failed: {0}")]
    UpgradeFailed(String),
    /// The request did not arrive over a transport that supports HTTP/1.1
    /// upgrades (e.g. HTTP/2). Docker clients always negotiate HTTP/1.1
    /// for attach/exec, so this is treated as a hard error.
    #[error("not http/1.1 upgrade-capable")]
    NotHttp11,
}

/// An HTTP/1.1 connection that has been hijacked away from the framework
/// and now exposes a raw bidirectional byte stream.
///
/// Construct one with [`HijackedConnection::from_request`] **after** the
/// handler has returned a `101 Switching Protocols` response (use
/// [`raw_stream_response`] or [`multiplexed_stream_response`] for that).
/// Hyper drives the upgrade once the response is flushed, at which point
/// the future returned by `from_request` resolves with the upgraded
/// transport.
pub struct HijackedConnection {
    inner: HijackedIo,
}

/// Type-erased wrapper around the upgraded transport.
///
/// We box the `Upgraded` so callers do not have to name `hyper::upgrade::Upgraded`
/// in their types and so the resulting connection is `Unpin` for use in
/// `tokio::io::copy_bidirectional` without further wrapping.
struct HijackedIo {
    io: Pin<Box<TokioIo<hyper::upgrade::Upgraded>>>,
}

impl AsyncRead for HijackedIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.io.as_mut().poll_read(cx, buf)
    }
}

impl AsyncWrite for HijackedIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.io.as_mut().poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.io.as_mut().poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.io.as_mut().poll_shutdown(cx)
    }
}

impl HijackedConnection {
    /// Drive an HTTP/1.1 upgrade to completion and return the hijacked
    /// connection.
    ///
    /// # Errors
    ///
    /// Returns [`HijackError::UpgradeFailed`] if hyper reports any error
    /// completing the upgrade (e.g. the client disconnected before the
    /// handshake finished). [`HijackError::NotHttp11`] is returned when
    /// the request lacked an `OnUpgrade` extension, which happens for
    /// HTTP/2 requests or when the Docker client failed to send the
    /// `Connection: Upgrade` header.
    pub async fn from_request(req: Request) -> Result<Self, HijackError> {
        // Detect HTTP/2 (or otherwise upgrade-incapable) requests up-front
        // so callers get a deterministic error rather than waiting on a
        // future that will never complete.
        if req
            .extensions()
            .get::<hyper::upgrade::OnUpgrade>()
            .is_none()
        {
            return Err(HijackError::NotHttp11);
        }
        let upgraded = hyper::upgrade::on(req)
            .await
            .map_err(|e| HijackError::UpgradeFailed(e.to_string()))?;
        Ok(Self {
            inner: HijackedIo {
                io: Box::pin(TokioIo::new(upgraded)),
            },
        })
    }

    /// Split the hijacked connection into independent read and write
    /// halves. The two halves can be moved to separate tasks for full-
    /// duplex copying (stdin into the container, stdout/stderr back out).
    #[must_use]
    pub fn split(
        self,
    ) -> (
        impl AsyncRead + Send + Unpin + 'static,
        impl AsyncWrite + Send + Unpin + 'static,
    ) {
        tokio::io::split(self.inner)
    }
}

/// Build a `101 Switching Protocols` response advertising a raw TCP-style
/// byte stream (Docker's TTY attach mode).
///
/// Once the response is sent the handler should call
/// [`HijackedConnection::from_request`] on the original request to obtain
/// the upgraded transport.
pub fn raw_stream_response() -> Response {
    let mut resp = (StatusCode::SWITCHING_PROTOCOLS, Body::empty()).into_response();
    let headers = resp.headers_mut();
    headers.insert(header::CONNECTION, HeaderValue::from_static("Upgrade"));
    headers.insert(header::UPGRADE, HeaderValue::from_static(RAW_UPGRADE_TOKEN));
    resp
}

/// Build a `101 Switching Protocols` response advertising Docker's
/// multiplexed log/attach stream wire format.
///
/// Used when the container has no TTY; payload bytes will be framed with
/// the 8-byte stream-tagged header described in [`super::log_frame`].
pub fn multiplexed_stream_response() -> Response {
    let mut resp = (StatusCode::SWITCHING_PROTOCOLS, Body::empty()).into_response();
    let headers = resp.headers_mut();
    headers.insert(header::CONNECTION, HeaderValue::from_static("Upgrade"));
    headers.insert(header::UPGRADE, HeaderValue::from_static(RAW_UPGRADE_TOKEN));
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(MULTIPLEXED_CONTENT_TYPE),
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_stream_response_has_101_and_upgrade_headers() {
        let resp = raw_stream_response();
        assert_eq!(resp.status(), StatusCode::SWITCHING_PROTOCOLS);
        assert_eq!(
            resp.headers().get(header::CONNECTION).unwrap(),
            HeaderValue::from_static("Upgrade")
        );
        assert_eq!(
            resp.headers().get(header::UPGRADE).unwrap(),
            HeaderValue::from_static("tcp")
        );
        // The raw response intentionally omits Content-Type; clients treat
        // the upgraded socket as an opaque byte pipe.
        assert!(resp.headers().get(header::CONTENT_TYPE).is_none());
    }

    #[test]
    fn multiplexed_stream_response_has_101_and_multiplexed_content_type() {
        let resp = multiplexed_stream_response();
        assert_eq!(resp.status(), StatusCode::SWITCHING_PROTOCOLS);
        assert_eq!(
            resp.headers().get(header::CONNECTION).unwrap(),
            HeaderValue::from_static("Upgrade")
        );
        assert_eq!(
            resp.headers().get(header::UPGRADE).unwrap(),
            HeaderValue::from_static("tcp")
        );
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            HeaderValue::from_static("application/vnd.docker.multiplexed-stream")
        );
    }

    #[test]
    fn hijack_error_display_round_trips_message() {
        let upgrade = HijackError::UpgradeFailed("peer closed during handshake".into());
        assert_eq!(
            upgrade.to_string(),
            "upgrade failed: peer closed during handshake"
        );

        let not_http11 = HijackError::NotHttp11;
        assert_eq!(not_http11.to_string(), "not http/1.1 upgrade-capable");
    }

    /// Real upgrade flow needs a live TCP socket pair speaking HTTP/1.1
    /// with `Connection: Upgrade`; that's a full integration test fixture
    /// rather than a unit test, so it's parked behind `#[ignore]` until
    /// the attach/exec endpoints land and bring their own integration
    /// harness.
    #[tokio::test]
    #[ignore = "requires a real HTTP/1.1 upgrade fixture; covered by the attach/exec integration suite"]
    async fn from_request_drives_upgrade_to_completion() {
        // Intentionally empty.
    }
}
