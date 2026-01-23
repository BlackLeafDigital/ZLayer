//! Reverse proxy service implementation
//!
//! This module provides the core proxy service that handles request forwarding.

use crate::config::ProxyConfig;
use crate::error::{ProxyError, Result};
use crate::lb::ConnectionGuard;
use crate::routing::Router;
use bytes::Bytes;
use http::{header, Request, Response, Uri, Version};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use std::net::SocketAddr;
use std::sync::Arc;
use std::task::{Context, Poll};
use tower::Service;
use tracing::{debug, error, info};

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
    /// Router for matching requests
    router: Arc<Router>,
    /// HTTP client for backend requests
    client: Client<hyper_util::client::legacy::connect::HttpConnector, BoxBody>,
    /// Proxy configuration
    config: Arc<ProxyConfig>,
    /// Client remote address (set per-request)
    remote_addr: Option<SocketAddr>,
    /// Whether the connection is over TLS
    is_tls: bool,
}

impl ReverseProxyService {
    /// Create a new reverse proxy service
    pub fn new(router: Arc<Router>, config: Arc<ProxyConfig>) -> Self {
        let client = Client::builder(TokioExecutor::new())
            .http2_only(false)
            .build_http();

        Self {
            router,
            client,
            config,
            remote_addr: None,
            is_tls: false,
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

    /// Check if this connection is over TLS
    pub fn is_tls(&self) -> bool {
        self.is_tls
    }

    /// Handle an incoming HTTP request
    pub async fn proxy_request(&self, req: Request<Incoming>) -> Result<Response<BoxBody>> {
        let method = req.method().clone();
        let uri = req.uri().clone();

        // Extract host and path for routing
        let host = req
            .headers()
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .or_else(|| uri.host());

        let path = uri.path();

        debug!(
            method = %method,
            host = ?host,
            path = %path,
            "Routing request"
        );

        // Match route
        let route_match = self.router.match_route(host, path).await?;
        let route = &route_match.route;
        let lb = &route_match.load_balancer;

        // Select backend
        let backend = lb.select().await?;
        let _guard = ConnectionGuard::new(backend.clone());

        info!(
            method = %method,
            host = ?host,
            path = %path,
            backend = %backend.addr,
            service = %route.service,
            "Forwarding request"
        );

        // Build the forwarded request
        let forwarded_req = self
            .build_forwarded_request(req, &backend.addr, route)
            .await?;

        // Forward to backend
        let response = self.client.request(forwarded_req).await.map_err(|e| {
            error!(error = %e, backend = %backend.addr, "Backend request failed");
            ProxyError::BackendRequestFailed(e.to_string())
        })?;

        // Convert response body
        let (parts, body) = response.into_parts();
        let body = body
            .collect()
            .await
            .map_err(|e| ProxyError::BackendRequestFailed(e.to_string()))?
            .to_bytes();

        Ok(Response::from_parts(parts, full_body(body)))
    }

    async fn build_forwarded_request(
        &self,
        req: Request<Incoming>,
        backend: &SocketAddr,
        route: &crate::routing::Route,
    ) -> Result<Request<BoxBody>> {
        let (mut parts, body) = req.into_parts();

        // Transform the path if needed
        let original_path = parts.uri.path();
        let transformed_path = route.transform_path(original_path);

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

        // Collect the incoming body
        let body_bytes = body
            .collect()
            .await
            .map_err(|e| ProxyError::BackendRequestFailed(e.to_string()))?
            .to_bytes();

        let req = Request::from_parts(parts, full_body(body_bytes));
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
}
