//! WebSocket and upgrade tunneling
//!
//! This module provides functionality for handling HTTP upgrade requests,
//! including WebSocket connections, and bidirectional tunneling.

use crate::error::{ProxyError, Result};
use http::{header, Request, Response, StatusCode};
use hyper::upgrade::OnUpgrade;
use tokio::io::{AsyncRead, AsyncWrite};
use tracing::{debug, error, info, warn};

/// Check if a request is an upgrade request
///
/// Returns true if the request has a Connection: upgrade header
pub fn is_upgrade_request<B>(req: &Request<B>) -> bool {
    req.headers()
        .get(header::CONNECTION)
        .and_then(|h| h.to_str().ok())
        .map(|v| {
            v.split(',')
                .any(|t| t.trim().eq_ignore_ascii_case("upgrade"))
        })
        .unwrap_or(false)
}

/// Check if a request is a WebSocket upgrade request
///
/// Returns true if the request has both Connection: upgrade and Upgrade: websocket headers
pub fn is_websocket_upgrade<B>(req: &Request<B>) -> bool {
    if !is_upgrade_request(req) {
        return false;
    }

    req.headers()
        .get(header::UPGRADE)
        .and_then(|h| h.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false)
}

/// Get the upgrade protocol from a request
///
/// Returns the value of the Upgrade header if present
pub fn get_upgrade_protocol<B>(req: &Request<B>) -> Option<&str> {
    req.headers()
        .get(header::UPGRADE)
        .and_then(|h| h.to_str().ok())
}

/// Check if a response indicates a successful upgrade
pub fn is_upgrade_response<B>(res: &Response<B>) -> bool {
    res.status() == StatusCode::SWITCHING_PROTOCOLS
}

/// Proxy a tunnel connection between two upgraded connections
///
/// This function performs bidirectional copying between the client
/// and server connections after an upgrade.
pub async fn proxy_tunnel<C, S>(mut client: C, mut server: S) -> Result<()>
where
    C: AsyncRead + AsyncWrite + Unpin + Send,
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    match tokio::io::copy_bidirectional(&mut client, &mut server).await {
        Ok((client_to_server, server_to_client)) => {
            debug!(
                client_to_server = client_to_server,
                server_to_client = server_to_client,
                "Tunnel closed"
            );
            Ok(())
        }
        Err(e) => {
            // Connection reset is common and not really an error
            if e.kind() == std::io::ErrorKind::ConnectionReset {
                debug!("Tunnel connection reset");
                Ok(())
            } else {
                warn!(error = %e, "Tunnel error");
                Err(ProxyError::Io(e))
            }
        }
    }
}

/// Handle upgrade with explicit upgrade futures
///
/// This is a higher-level function that takes OnUpgrade futures from hyper
/// and handles the bidirectional copying between them.
pub async fn proxy_upgrade(client_upgrade: OnUpgrade, server_upgrade: OnUpgrade) -> Result<()> {
    // Wait for both upgrades to complete
    let (client_result, server_result) = tokio::join!(client_upgrade, server_upgrade);

    let client_io = client_result.map_err(|e| {
        error!(error = %e, "Client upgrade failed");
        ProxyError::Internal(format!("Client upgrade failed: {}", e))
    })?;

    let server_io = server_result.map_err(|e| {
        error!(error = %e, "Server upgrade failed");
        ProxyError::Internal(format!("Server upgrade failed: {}", e))
    })?;

    info!("Upgrade successful, starting bidirectional tunnel");

    // Use hyper_util's TokioIo wrapper for the upgraded connections
    use hyper_util::rt::TokioIo;
    let client = TokioIo::new(client_io);
    let server = TokioIo::new(server_io);

    proxy_tunnel(client, server).await
}

/// Extract WebSocket key from a request for validation
pub fn get_websocket_key<B>(req: &Request<B>) -> Option<&str> {
    req.headers()
        .get("sec-websocket-key")
        .and_then(|h| h.to_str().ok())
}

/// Extract WebSocket version from a request
pub fn get_websocket_version<B>(req: &Request<B>) -> Option<&str> {
    req.headers()
        .get("sec-websocket-version")
        .and_then(|h| h.to_str().ok())
}

/// Headers that should be forwarded for WebSocket upgrades
const WEBSOCKET_HEADERS: &[&str] = &[
    "sec-websocket-key",
    "sec-websocket-version",
    "sec-websocket-protocol",
    "sec-websocket-extensions",
];

/// Check if a header should be preserved for WebSocket upgrades
pub fn is_websocket_header(name: &str) -> bool {
    WEBSOCKET_HEADERS
        .iter()
        .any(|h| h.eq_ignore_ascii_case(name))
}

/// Copy upgrade-related headers from source to destination request parts
pub fn copy_upgrade_headers(src: &http::request::Parts, dst: &mut http::request::Parts) {
    // Copy Connection and Upgrade headers
    if let Some(conn) = src.headers.get(header::CONNECTION) {
        dst.headers.insert(header::CONNECTION, conn.clone());
    }
    if let Some(upgrade) = src.headers.get(header::UPGRADE) {
        dst.headers.insert(header::UPGRADE, upgrade.clone());
    }

    // Copy WebSocket-specific headers
    for header_name in WEBSOCKET_HEADERS {
        if let Some(value) = src.headers.get(*header_name) {
            if let Ok(name) = header::HeaderName::from_bytes(header_name.as_bytes()) {
                dst.headers.insert(name, value.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Request;
    use tokio::io::{AsyncRead, AsyncWrite};

    #[test]
    fn test_is_upgrade_request() {
        // With upgrade
        let req = Request::builder()
            .header("Connection", "upgrade")
            .body(())
            .unwrap();
        assert!(is_upgrade_request(&req));

        // With mixed case
        let req = Request::builder()
            .header("connection", "Upgrade")
            .body(())
            .unwrap();
        assert!(is_upgrade_request(&req));

        // With multiple values
        let req = Request::builder()
            .header("Connection", "keep-alive, upgrade")
            .body(())
            .unwrap();
        assert!(is_upgrade_request(&req));

        // Without upgrade
        let req = Request::builder()
            .header("Connection", "keep-alive")
            .body(())
            .unwrap();
        assert!(!is_upgrade_request(&req));

        // No connection header
        let req = Request::builder().body(()).unwrap();
        assert!(!is_upgrade_request(&req));
    }

    #[test]
    fn test_is_websocket_upgrade() {
        // Valid WebSocket upgrade
        let req = Request::builder()
            .header("Connection", "upgrade")
            .header("Upgrade", "websocket")
            .body(())
            .unwrap();
        assert!(is_websocket_upgrade(&req));

        // Mixed case
        let req = Request::builder()
            .header("Connection", "Upgrade")
            .header("Upgrade", "WebSocket")
            .body(())
            .unwrap();
        assert!(is_websocket_upgrade(&req));

        // Missing Upgrade header
        let req = Request::builder()
            .header("Connection", "upgrade")
            .body(())
            .unwrap();
        assert!(!is_websocket_upgrade(&req));

        // Wrong upgrade protocol
        let req = Request::builder()
            .header("Connection", "upgrade")
            .header("Upgrade", "h2c")
            .body(())
            .unwrap();
        assert!(!is_websocket_upgrade(&req));
    }

    #[test]
    fn test_get_upgrade_protocol() {
        let req = Request::builder()
            .header("Upgrade", "websocket")
            .body(())
            .unwrap();
        assert_eq!(get_upgrade_protocol(&req), Some("websocket"));

        let req = Request::builder()
            .header("Upgrade", "h2c")
            .body(())
            .unwrap();
        assert_eq!(get_upgrade_protocol(&req), Some("h2c"));

        let req = Request::builder().body(()).unwrap();
        assert_eq!(get_upgrade_protocol(&req), None);
    }

    #[test]
    fn test_is_upgrade_response() {
        let res = Response::builder()
            .status(StatusCode::SWITCHING_PROTOCOLS)
            .body(())
            .unwrap();
        assert!(is_upgrade_response(&res));

        let res = Response::builder().status(StatusCode::OK).body(()).unwrap();
        assert!(!is_upgrade_response(&res));
    }

    #[test]
    fn test_get_websocket_key() {
        let req = Request::builder()
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .body(())
            .unwrap();
        assert_eq!(get_websocket_key(&req), Some("dGhlIHNhbXBsZSBub25jZQ=="));

        let req = Request::builder().body(()).unwrap();
        assert_eq!(get_websocket_key(&req), None);
    }

    #[test]
    fn test_get_websocket_version() {
        let req = Request::builder()
            .header("sec-websocket-version", "13")
            .body(())
            .unwrap();
        assert_eq!(get_websocket_version(&req), Some("13"));
    }

    #[test]
    fn test_is_websocket_header() {
        assert!(is_websocket_header("sec-websocket-key"));
        assert!(is_websocket_header("Sec-WebSocket-Key"));
        assert!(is_websocket_header("sec-websocket-version"));
        assert!(is_websocket_header("sec-websocket-protocol"));
        assert!(is_websocket_header("sec-websocket-extensions"));
        assert!(!is_websocket_header("content-type"));
        assert!(!is_websocket_header("host"));
    }

    #[test]
    fn test_copy_upgrade_headers() {
        let src = Request::builder()
            .header("Connection", "upgrade")
            .header("Upgrade", "websocket")
            .header("sec-websocket-key", "test-key")
            .header("sec-websocket-version", "13")
            .header("content-type", "text/plain")
            .body(())
            .unwrap()
            .into_parts()
            .0;

        let mut dst = Request::builder().body(()).unwrap().into_parts().0;

        copy_upgrade_headers(&src, &mut dst);

        assert!(dst.headers.get(header::CONNECTION).is_some());
        assert!(dst.headers.get(header::UPGRADE).is_some());
        assert!(dst.headers.get("sec-websocket-key").is_some());
        assert!(dst.headers.get("sec-websocket-version").is_some());
        // content-type should not be copied
        assert!(dst.headers.get("content-type").is_none());
    }

    #[tokio::test]
    async fn test_proxy_tunnel_connection_reset() {
        use std::io::{Error, ErrorKind};

        // Test that connection reset is handled gracefully
        // This simulates what happens when a connection is closed unexpectedly

        // Create a mock stream that immediately returns connection reset
        struct MockStream;

        impl AsyncRead for MockStream {
            fn poll_read(
                self: std::pin::Pin<&mut Self>,
                _cx: &mut std::task::Context<'_>,
                _buf: &mut tokio::io::ReadBuf<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Err(Error::new(
                    ErrorKind::ConnectionReset,
                    "connection reset",
                )))
            }
        }

        impl AsyncWrite for MockStream {
            fn poll_write(
                self: std::pin::Pin<&mut Self>,
                _cx: &mut std::task::Context<'_>,
                buf: &[u8],
            ) -> std::task::Poll<std::io::Result<usize>> {
                std::task::Poll::Ready(Ok(buf.len()))
            }

            fn poll_flush(
                self: std::pin::Pin<&mut Self>,
                _cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Ok(()))
            }

            fn poll_shutdown(
                self: std::pin::Pin<&mut Self>,
                _cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Ok(()))
            }
        }

        let client = MockStream;
        let server = MockStream;

        // Connection reset should be handled gracefully (returns Ok)
        let result = proxy_tunnel(client, server).await;
        assert!(result.is_ok());
    }
}
