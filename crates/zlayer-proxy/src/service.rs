//! Reverse proxy service implementation
//!
//! This module provides the core proxy service that handles request forwarding.
//! It uses the `ServiceRegistry` for route resolution and backend selection.

use crate::acme::CertManager;
use crate::config::ProxyConfig;
use crate::error::{ProxyError, Result};
use crate::lb::LoadBalancer;
use crate::routes::{transform_path, ResolvedService, ServiceRegistry};
use bytes::Bytes;
use http::{header, Request, Response, Uri, Version};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::upgrade::OnUpgrade;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::net::TcpStream;
use tower::Service;
use tracing::{debug, error, info, warn};
use zlayer_spec::ExposeType;

/// The overlay network CIDR used for internal service communication.
/// Source IPs outside this range are rejected for internal-only routes.
const OVERLAY_NETWORK: (u8, u8) = (10, 200); // 10.200.0.0/16

/// Check whether an IP address belongs to the overlay network (10.200.0.0/16).
fn is_overlay_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            octets[0] == OVERLAY_NETWORK.0 && octets[1] == OVERLAY_NETWORK.1
        }
        IpAddr::V6(_) => false,
    }
}

/// Body type for outgoing responses
pub type BoxBody = http_body_util::combinators::BoxBody<Bytes, hyper::Error>;

/// Empty body utility
pub fn empty_body() -> BoxBody {
    http_body_util::Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed()
}

/// Full body utility
pub fn full_body(bytes: impl Into<Bytes>) -> BoxBody {
    Full::new(bytes.into())
        .map_err(|never| match never {})
        .boxed()
}

/// The reverse proxy service
#[derive(Clone)]
pub struct ReverseProxyService {
    /// Service registry for route resolution
    registry: Arc<ServiceRegistry>,
    /// Load balancer for backend selection
    load_balancer: Arc<LoadBalancer>,
    /// HTTP client for backend requests
    client: Client<hyper_util::client::legacy::connect::HttpConnector, BoxBody>,
    /// Proxy configuration
    config: Arc<ProxyConfig>,
    /// Client remote address (set per-request)
    remote_addr: Option<SocketAddr>,
    /// Whether the connection is over TLS
    is_tls: bool,
    /// Certificate manager for ACME challenge responses
    cert_manager: Option<Arc<CertManager>>,
}

impl ReverseProxyService {
    /// Create a new reverse proxy service
    pub fn new(
        registry: Arc<ServiceRegistry>,
        load_balancer: Arc<LoadBalancer>,
        config: Arc<ProxyConfig>,
    ) -> Self {
        let client = Client::builder(TokioExecutor::new())
            .pool_max_idle_per_host(config.pool.max_idle_per_backend)
            .pool_idle_timeout(config.pool.idle_timeout)
            .pool_timer(hyper_util::rt::TokioTimer::new())
            .build_http();

        Self {
            registry,
            load_balancer,
            client,
            config,
            remote_addr: None,
            is_tls: false,
            cert_manager: None,
        }
    }

    /// Set the remote client address for this request
    pub fn with_remote_addr(mut self, addr: SocketAddr) -> Self {
        self.remote_addr = Some(addr);
        self
    }

    /// Mark this connection as being over TLS
    pub fn with_tls(mut self, is_tls: bool) -> Self {
        self.is_tls = is_tls;
        self
    }

    /// Set the certificate manager for ACME challenge interception
    pub fn with_cert_manager(mut self, cm: Arc<CertManager>) -> Self {
        self.cert_manager = Some(cm);
        self
    }

    /// Check if this connection is over TLS
    pub fn is_tls(&self) -> bool {
        self.is_tls
    }

    /// Handle an incoming HTTP request
    pub async fn proxy_request(&self, mut req: Request<Incoming>) -> Result<Response<BoxBody>> {
        let start = std::time::Instant::now();
        let method = req.method().clone();
        let uri = req.uri().clone();

        let host = req
            .headers()
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .or_else(|| uri.host())
            .map(|h| h.to_string());

        let path = uri.path().to_string();

        // ACME HTTP-01 challenge interception
        if path.starts_with("/.well-known/acme-challenge/") {
            if let Some(token) = path.strip_prefix("/.well-known/acme-challenge/") {
                if !token.is_empty() {
                    if let Some(ref cm) = self.cert_manager {
                        if let Some(auth) = cm.get_challenge_response(token).await {
                            return Ok(Response::builder()
                                .status(200)
                                .header("content-type", "text/plain")
                                .body(full_body(auth))
                                .unwrap());
                        }
                    }
                }
            }
        }

        // Check for WebSocket/HTTP upgrade
        if crate::tunnel::is_upgrade_request(&req) {
            // Resolve to get backend for upgrade
            let resolved = self
                .registry
                .resolve(host.as_deref(), &path)
                .await
                .ok_or_else(|| ProxyError::RouteNotFound {
                    host: host.as_deref().unwrap_or("<none>").to_string(),
                    path: path.clone(),
                })?;

            // Enforce internal endpoints
            if resolved.expose == ExposeType::Internal {
                if let Some(addr) = self.remote_addr {
                    if !is_overlay_ip(addr.ip()) {
                        return Err(ProxyError::Forbidden(
                            "endpoint is internal-only".to_string(),
                        ));
                    }
                }
            }

            let backend = self
                .load_balancer
                .select(&resolved.name)
                .ok_or_else(|| ProxyError::NoHealthyBackends {
                    service: resolved.name.clone(),
                })?;
            let _guard = backend.track_connection();
            let backend_addr = backend.addr;

            info!(
                method = %method,
                host = ?host,
                path = %path,
                backend = %backend_addr,
                service = %resolved.name,
                "Forwarding upgrade request"
            );

            // Extract the client's OnUpgrade future BEFORE consuming the request
            let client_upgrade: OnUpgrade = hyper::upgrade::on(&mut req);

            // Build the backend URI
            let original_path = req.uri().path();
            let transformed_path =
                transform_path(&resolved.path_prefix, original_path, resolved.strip_prefix);
            let new_uri = format!(
                "http://{}{}{}",
                backend_addr,
                transformed_path,
                req.uri()
                    .query()
                    .map(|q| format!("?{}", q))
                    .unwrap_or_default()
            );

            // Build backend request, preserving upgrade headers
            let (orig_parts, _body) = req.into_parts();
            let mut backend_parts = http::request::Builder::new()
                .method(orig_parts.method.clone())
                .uri(
                    new_uri
                        .parse::<Uri>()
                        .map_err(|e| ProxyError::InvalidRequest(format!("Invalid URI: {}", e)))?,
                )
                .body(())
                .unwrap()
                .into_parts()
                .0;

            // Copy all original headers first (preserving Host, etc.)
            for (name, value) in orig_parts.headers.iter() {
                backend_parts.headers.insert(name.clone(), value.clone());
            }

            // Copy upgrade-specific headers (Connection, Upgrade, Sec-WebSocket-*)
            crate::tunnel::copy_upgrade_headers(&orig_parts, &mut backend_parts);

            // Add forwarding headers
            self.add_forwarding_headers(&mut backend_parts);

            // Connect directly to backend (bypass connection pool for long-lived upgrades)
            let tcp_stream = TcpStream::connect(backend_addr).await.map_err(|e| {
                error!(error = %e, backend = %backend_addr, "Backend upgrade connect failed");
                ProxyError::BackendConnectionFailed {
                    backend: backend_addr,
                    reason: e.to_string(),
                }
            })?;
            let io = TokioIo::new(tcp_stream);

            // Perform HTTP/1.1 handshake preserving header case
            let (mut sender, conn) = hyper::client::conn::http1::Builder::new()
                .preserve_header_case(true)
                .handshake(io)
                .await
                .map_err(|e| {
                    error!(error = %e, backend = %backend_addr, "Backend upgrade handshake failed");
                    ProxyError::BackendRequestFailed(format!("Upgrade handshake failed: {}", e))
                })?;

            // Spawn the connection driver
            tokio::spawn(async move {
                if let Err(e) = conn.with_upgrades().await {
                    error!(error = %e, "Backend upgrade connection driver error");
                }
            });

            // Send the request to the backend
            let backend_req =
                Request::from_parts(backend_parts, http_body_util::Empty::<Bytes>::new());
            let backend_response = sender.send_request(backend_req).await.map_err(|e| {
                error!(error = %e, backend = %backend_addr, "Backend upgrade request failed");
                ProxyError::BackendRequestFailed(e.to_string())
            })?;

            if backend_response.status() == http::StatusCode::SWITCHING_PROTOCOLS {
                // Get the server's OnUpgrade future
                let server_upgrade: OnUpgrade = hyper::upgrade::on(backend_response);

                // Build 101 response to send back to the client
                let mut resp_builder = Response::builder().status(http::StatusCode::SWITCHING_PROTOCOLS);
                // Note: we need to construct the response manually since we consumed
                // the backend response to get OnUpgrade. Copy relevant headers.
                // The hyper::upgrade::on() for the response does NOT consume it —
                // it was consumed. We need to return a 101 with appropriate headers.
                // Actually, hyper::upgrade::on() takes the response by value, so we
                // must build our own 101 response for the client.

                // For the client response, set Connection: upgrade and Upgrade headers
                if let Some(upgrade_val) = orig_parts.headers.get(header::UPGRADE) {
                    resp_builder = resp_builder.header(header::UPGRADE, upgrade_val.clone());
                }
                resp_builder = resp_builder.header(header::CONNECTION, "upgrade");

                let client_response = resp_builder
                    .body(empty_body())
                    .map_err(|e| ProxyError::Internal(format!("Failed to build 101 response: {}", e)))?;

                // Spawn background task to bridge the upgraded connections
                tokio::spawn(async move {
                    if let Err(e) = crate::tunnel::proxy_upgrade(client_upgrade, server_upgrade).await
                    {
                        debug!(error = %e, "Upgrade tunnel ended");
                    }
                });

                // Add timing header to the 101 response
                let (mut parts, body) = client_response.into_parts();
                if let Ok(hv) = format!("proxy;dur={}", start.elapsed().as_millis()).parse() {
                    parts.headers.insert("server-timing", hv);
                }

                return Ok(Response::from_parts(parts, body));
            } else {
                // Backend didn't upgrade — stream the response as-is
                let (mut parts, body) = backend_response.into_parts();
                let streaming_body: BoxBody = body.map_err(|e: hyper::Error| e).boxed();

                // Add HSTS header for TLS connections
                if self.is_tls && self.config.headers.hsts {
                    let value = if self.config.headers.hsts_subdomains {
                        format!(
                            "max-age={}; includeSubDomains",
                            self.config.headers.hsts_max_age
                        )
                    } else {
                        format!("max-age={}", self.config.headers.hsts_max_age)
                    };
                    if let Ok(hv) = value.parse() {
                        parts.headers.insert("strict-transport-security", hv);
                    }
                }

                // Add Server-Timing header
                if let Ok(hv) = format!("proxy;dur={}", start.elapsed().as_millis()).parse() {
                    parts.headers.insert("server-timing", hv);
                }

                return Ok(Response::from_parts(parts, streaming_body));
            }
        }

        debug!(method = %method, host = ?host, path = %path, "Routing request");

        // Resolve route
        let resolved = self
            .registry
            .resolve(host.as_deref(), &path)
            .await
            .ok_or_else(|| ProxyError::RouteNotFound {
                host: host.as_deref().unwrap_or("<none>").to_string(),
                path: path.clone(),
            })?;

        // Enforce internal endpoints
        if resolved.expose == ExposeType::Internal {
            match self.remote_addr {
                Some(addr) if !is_overlay_ip(addr.ip()) => {
                    warn!(
                        source = %addr.ip(),
                        service = %resolved.name,
                        "Rejected non-overlay source for internal endpoint"
                    );
                    return Err(ProxyError::Forbidden(
                        "endpoint is internal-only".to_string(),
                    ));
                }
                None => {
                    debug!(
                        service = %resolved.name,
                        "No remote_addr available; skipping overlay source check"
                    );
                }
                _ => {}
            }
        }

        // Select backend via load balancer
        let backend = self
            .load_balancer
            .select(&resolved.name)
            .ok_or_else(|| ProxyError::NoHealthyBackends {
                service: resolved.name.clone(),
            })?;
        let _guard = backend.track_connection();
        let backend_addr = backend.addr;

        info!(
            method = %method,
            host = ?host,
            path = %path,
            backend = %backend_addr,
            service = %resolved.name,
            "Forwarding request"
        );

        // Build forwarded request
        let forwarded_req = self
            .build_forwarded_request(req, &backend_addr, &resolved)
            .await?;

        // Forward to backend
        let response = self.client.request(forwarded_req).await.map_err(|e| {
            error!(error = %e, backend = %backend_addr, "Backend request failed");
            ProxyError::BackendRequestFailed(e.to_string())
        })?;

        let (mut parts, body) = response.into_parts();
        let streaming_body: BoxBody = body.map_err(|e: hyper::Error| e).boxed();

        // Add HSTS header for TLS connections
        if self.is_tls && self.config.headers.hsts {
            let value = if self.config.headers.hsts_subdomains {
                format!(
                    "max-age={}; includeSubDomains",
                    self.config.headers.hsts_max_age
                )
            } else {
                format!("max-age={}", self.config.headers.hsts_max_age)
            };
            if let Ok(hv) = value.parse() {
                parts.headers.insert("strict-transport-security", hv);
            }
        }

        // Add Server-Timing header
        if let Ok(hv) = format!("proxy;dur={}", start.elapsed().as_millis()).parse() {
            parts.headers.insert("server-timing", hv);
        }

        Ok(Response::from_parts(parts, streaming_body))
    }

    async fn build_forwarded_request(
        &self,
        req: Request<Incoming>,
        backend: &SocketAddr,
        resolved: &ResolvedService,
    ) -> Result<Request<BoxBody>> {
        let (mut parts, body) = req.into_parts();

        // Transform the path if needed
        let original_path = parts.uri.path();
        let transformed_path =
            transform_path(&resolved.path_prefix, original_path, resolved.strip_prefix);

        // Build new URI for backend
        let new_uri = format!(
            "http://{}{}{}",
            backend,
            transformed_path,
            parts
                .uri
                .query()
                .map(|q| format!("?{}", q))
                .unwrap_or_default()
        );

        parts.uri = new_uri
            .parse::<Uri>()
            .map_err(|e| ProxyError::InvalidRequest(format!("Invalid URI: {}", e)))?;

        // Add forwarding headers
        self.add_forwarding_headers(&mut parts);

        // Remove hop-by-hop headers
        Self::remove_hop_by_hop_headers(&mut parts);

        let streaming_body: BoxBody = body.map_err(|e: hyper::Error| e).boxed();

        let req = Request::from_parts(parts, streaming_body);
        Ok(req)
    }

    fn add_forwarding_headers(&self, parts: &mut http::request::Parts) {
        let config = &self.config.headers;

        // X-Forwarded-For
        if config.x_forwarded_for {
            if let Some(addr) = self.remote_addr {
                let existing = parts
                    .headers
                    .get("x-forwarded-for")
                    .and_then(|h| h.to_str().ok())
                    .map(|s| format!("{}, {}", s, addr.ip()))
                    .unwrap_or_else(|| addr.ip().to_string());

                if let Ok(value) = existing.parse() {
                    parts.headers.insert("x-forwarded-for", value);
                }
            }
        }

        // X-Forwarded-Proto
        if config.x_forwarded_proto && parts.headers.get("x-forwarded-proto").is_none() {
            let proto = if self.is_tls { "https" } else { "http" };
            if let Ok(value) = proto.parse() {
                parts.headers.insert("x-forwarded-proto", value);
            }
        }

        // X-Forwarded-Host
        if config.x_forwarded_host {
            if let Some(host) = parts.headers.get(header::HOST).cloned() {
                if parts.headers.get("x-forwarded-host").is_none() {
                    parts.headers.insert("x-forwarded-host", host);
                }
            }
        }

        // X-Real-IP
        if config.x_real_ip {
            if let Some(addr) = self.remote_addr {
                if parts.headers.get("x-real-ip").is_none() {
                    if let Ok(value) = addr.ip().to_string().parse() {
                        parts.headers.insert("x-real-ip", value);
                    }
                }
            }
        }

        // Via header
        if config.via {
            let proto_version = match parts.version {
                Version::HTTP_09 => "0.9",
                Version::HTTP_10 => "1.0",
                Version::HTTP_11 => "1.1",
                Version::HTTP_2 => "2.0",
                Version::HTTP_3 => "3.0",
                _ => "1.1",
            };

            let via_value = format!("{} {}", proto_version, config.server_name);
            let existing = parts
                .headers
                .get(header::VIA)
                .and_then(|h| h.to_str().ok())
                .map(|s| format!("{}, {}", s, via_value))
                .unwrap_or(via_value);

            if let Ok(value) = existing.parse() {
                parts.headers.insert(header::VIA, value);
            }
        }
    }

    fn remove_hop_by_hop_headers(parts: &mut http::request::Parts) {
        // First, collect headers listed in the Connection header before we remove it
        let connection_headers: Vec<String> = parts
            .headers
            .get(header::CONNECTION)
            .and_then(|h| h.to_str().ok())
            .map(|value| value.split(',').map(|s| s.trim().to_lowercase()).collect())
            .unwrap_or_default();

        // Standard hop-by-hop headers that should not be forwarded
        const HOP_BY_HOP: &[&str] = &[
            "connection",
            "keep-alive",
            "proxy-authenticate",
            "proxy-authorization",
            "te",
            "trailer",
            "transfer-encoding",
            "upgrade",
        ];

        for header_name in HOP_BY_HOP {
            parts.headers.remove(*header_name);
        }

        // Also remove headers that were listed in the Connection header
        for header_name in connection_headers {
            parts.headers.remove(header_name.as_str());
        }
    }

    /// Create an error response
    pub fn error_response(error: &ProxyError) -> Response<BoxBody> {
        let status = error.status_code();
        let body = format!("{{\"error\": \"{}\"}}", error);

        Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "application/json")
            .body(full_body(body))
            .unwrap()
    }
}

impl Service<Request<Incoming>> for ReverseProxyService {
    type Response = Response<BoxBody>;
    type Error = ProxyError;
    type Future = std::pin::Pin<
        Box<
            dyn std::future::Future<Output = std::result::Result<Self::Response, Self::Error>>
                + Send,
        >,
    >;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<std::result::Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<Incoming>) -> Self::Future {
        let this = self.clone();
        Box::pin(async move { this.proxy_request(req).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_response() {
        let error = ProxyError::RouteNotFound {
            host: "example.com".to_string(),
            path: "/api".to_string(),
        };

        let response = ReverseProxyService::error_response(&error);
        assert_eq!(response.status(), http::StatusCode::NOT_FOUND);
    }

    #[test]
    fn test_hop_by_hop_headers() {
        let mut parts = http::request::Builder::new()
            .method("GET")
            .uri("/test")
            .header("connection", "keep-alive, x-custom")
            .header("keep-alive", "timeout=5")
            .header("x-custom", "value")
            .header("x-other", "value")
            .body(())
            .unwrap()
            .into_parts()
            .0;

        ReverseProxyService::remove_hop_by_hop_headers(&mut parts);

        assert!(parts.headers.get("connection").is_none());
        assert!(parts.headers.get("keep-alive").is_none());
        assert!(parts.headers.get("x-custom").is_none());
        // x-other should remain
        assert!(parts.headers.get("x-other").is_some());
    }

    #[test]
    fn test_is_overlay_ip_accepts_overlay_range() {
        // 10.200.x.x should be recognized as overlay
        assert!(is_overlay_ip("10.200.0.1".parse().unwrap()));
        assert!(is_overlay_ip("10.200.255.254".parse().unwrap()));
        assert!(is_overlay_ip("10.200.1.100".parse().unwrap()));
    }

    #[test]
    fn test_is_overlay_ip_rejects_non_overlay() {
        // Non-overlay addresses
        assert!(!is_overlay_ip("192.168.1.1".parse().unwrap()));
        assert!(!is_overlay_ip("10.0.0.1".parse().unwrap()));
        assert!(!is_overlay_ip("10.201.0.1".parse().unwrap()));
        assert!(!is_overlay_ip("172.16.0.1".parse().unwrap()));
        assert!(!is_overlay_ip("8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn test_is_overlay_ip_rejects_ipv6() {
        assert!(!is_overlay_ip("::1".parse().unwrap()));
        assert!(!is_overlay_ip("fe80::1".parse().unwrap()));
    }

    #[test]
    fn test_forbidden_error_response() {
        let error = ProxyError::Forbidden("endpoint 'ws' is internal-only".to_string());
        let response = ReverseProxyService::error_response(&error);
        assert_eq!(response.status(), http::StatusCode::FORBIDDEN);
    }
}
