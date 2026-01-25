//! ZLayer Pingora-based reverse proxy implementation
//!
//! This module provides a high-performance reverse proxy built on Cloudflare's Pingora framework.
//! It supports:
//! - Host and path-based routing via ServiceRegistry
//! - Load balancing with health checks
//! - Request/response header manipulation
//! - TLS termination via CertManager

use async_trait::async_trait;
use dashmap::DashMap;
use pingora_core::upstreams::peer::HttpPeer;
use pingora_core::Result;
use pingora_http::{RequestHeader, ResponseHeader};
use pingora_load_balancing::selection::RoundRobin;
use pingora_load_balancing::{health_check, LoadBalancer};
use pingora_proxy::{ProxyHttp, Session};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::acme::CertManager;
use crate::routes::ServiceRegistry;

/// Per-request context for ZLayerProxy
///
/// This context is created for each incoming request and persists across all proxy phases.
pub struct ZLayerCtx {
    /// Name of the resolved service (for logging and metrics)
    pub service_name: Option<String>,
    /// Timestamp when request processing started
    pub start_time: Instant,
    /// Selected backend address as string (cached from upstream_peer)
    /// Uses String to avoid type conflicts between Pingora's SocketAddr and std::net::SocketAddr
    pub selected_backend: Option<String>,
}

impl Default for ZLayerCtx {
    fn default() -> Self {
        Self::new()
    }
}

impl ZLayerCtx {
    /// Create a new request context
    pub fn new() -> Self {
        Self {
            service_name: None,
            start_time: Instant::now(),
            selected_backend: None,
        }
    }

    /// Get elapsed time since request started
    pub fn elapsed(&self) -> Duration {
        self.start_time.elapsed()
    }
}

/// ZLayer reverse proxy built on Pingora
///
/// This proxy uses Pingora's high-performance HTTP proxy framework to route
/// requests to backend services. It integrates with:
/// - ServiceRegistry for routing decisions
/// - CertManager for TLS certificate management
/// - Per-service load balancers with health checks
pub struct ZLayerProxy {
    /// Service registry for resolving routes
    pub service_registry: Arc<ServiceRegistry>,
    /// Certificate manager for TLS
    pub cert_manager: Arc<CertManager>,
    /// Per-service load balancers (service_name -> LoadBalancer)
    pub load_balancers: Arc<DashMap<String, Arc<LoadBalancer<RoundRobin>>>>,
}

impl ZLayerProxy {
    /// Create a new ZLayerProxy
    pub fn new(service_registry: Arc<ServiceRegistry>, cert_manager: Arc<CertManager>) -> Self {
        Self {
            service_registry,
            cert_manager,
            load_balancers: Arc::new(DashMap::new()),
        }
    }

    /// Get or create a load balancer for a service
    ///
    /// This method creates a new load balancer with health checking if one doesn't exist.
    /// Load balancers are cached and reused across requests.
    pub fn get_or_create_lb(
        &self,
        service_name: &str,
        backends: &[SocketAddr],
    ) -> Arc<LoadBalancer<RoundRobin>> {
        // Check if we already have a load balancer for this service
        if let Some(lb) = self.load_balancers.get(service_name) {
            return lb.clone();
        }

        // Create backend strings for LoadBalancer
        let backend_strs: Vec<String> = backends.iter().map(|addr| addr.to_string()).collect();

        // Create new load balancer from backends
        let mut lb = if backend_strs.is_empty() {
            // Create empty load balancer - will fail selection but won't panic
            LoadBalancer::try_from_iter(std::iter::empty::<&str>()).unwrap_or_else(|_| {
                // Fallback: create with a dummy that will be filtered by health check
                LoadBalancer::try_from_iter(["0.0.0.0:0"]).unwrap()
            })
        } else {
            LoadBalancer::try_from_iter(backend_strs.iter().map(|s| s.as_str()))
                .expect("Failed to create load balancer from backends")
        };

        // Configure health checking
        let hc = health_check::TcpHealthCheck::new();
        lb.set_health_check(hc);
        lb.health_check_frequency = Some(Duration::from_secs(5));

        let lb = Arc::new(lb);
        self.load_balancers
            .insert(service_name.to_string(), lb.clone());

        tracing::debug!(
            service = %service_name,
            backends = ?backends,
            "Created load balancer for service"
        );

        lb
    }

    /// Update backends for an existing load balancer
    ///
    /// This method is called when service backends change (scale up/down).
    pub fn update_backends(&self, service_name: &str, backends: &[SocketAddr]) {
        // Remove old load balancer - next request will create a new one
        self.load_balancers.remove(service_name);

        // Pre-create the new load balancer
        if !backends.is_empty() {
            self.get_or_create_lb(service_name, backends);
        }

        tracing::info!(
            service = %service_name,
            backend_count = backends.len(),
            "Updated service backends"
        );
    }

    /// Remove a service's load balancer
    pub fn remove_service(&self, service_name: &str) {
        self.load_balancers.remove(service_name);
        tracing::info!(service = %service_name, "Removed service load balancer");
    }
}

#[async_trait]
impl ProxyHttp for ZLayerProxy {
    type CTX = ZLayerCtx;

    /// Create a new per-request context
    fn new_ctx(&self) -> Self::CTX {
        ZLayerCtx::new()
    }

    /// Select upstream peer for the request
    ///
    /// This is a REQUIRED method that determines which backend to route the request to.
    /// It performs:
    /// 1. Route resolution from host/path
    /// 2. Load balancer selection
    /// 3. Backend selection using round-robin
    async fn upstream_peer(
        &self,
        session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>> {
        // Extract host and path from request
        let req_header = session.req_header();
        let host = req_header
            .uri
            .host()
            .or_else(|| {
                req_header
                    .headers
                    .get("host")
                    .and_then(|h| h.to_str().ok())
                    .map(|h| h.split(':').next().unwrap_or(h))
            })
            .unwrap_or("localhost");
        let path = req_header.uri.path();

        tracing::debug!(host = %host, path = %path, "Resolving upstream for request");

        // Resolve service from registry
        let service = self.service_registry.resolve(host, path).map_err(|e| {
            tracing::warn!(host = %host, path = %path, error = %e, "Route not found");
            pingora_core::Error::new(pingora_core::ErrorType::HTTPStatus(502))
        })?;

        ctx.service_name = Some(service.name.clone());

        // Get or create load balancer for this service
        let lb = self.get_or_create_lb(&service.name, &service.backends);

        // Select a backend using round-robin
        // The hash parameter doesn't matter for round-robin, but we use path for consistency
        let backend = lb.select(path.as_bytes(), 256).ok_or_else(|| {
            tracing::error!(service = %service.name, "No healthy backends available");
            pingora_core::Error::new(pingora_core::ErrorType::HTTPStatus(503))
        })?;

        ctx.selected_backend = Some(backend.addr.to_string());

        tracing::debug!(
            service = %service.name,
            backend = %backend.addr,
            use_tls = service.use_tls,
            "Selected upstream backend"
        );

        // Create HttpPeer for the selected backend
        let peer = Box::new(HttpPeer::new(
            backend.addr,
            service.use_tls,
            service.sni_hostname.clone(),
        ));

        Ok(peer)
    }

    /// Modify the request before sending to upstream
    ///
    /// This filter adds standard proxy headers:
    /// - X-Forwarded-For: Client IP address
    /// - X-Forwarded-Proto: Original protocol (http/https)
    /// - X-Forwarded-Host: Original host header
    /// - X-Real-IP: Client IP address
    async fn upstream_request_filter(
        &self,
        session: &mut Session,
        upstream_request: &mut RequestHeader,
        _ctx: &mut Self::CTX,
    ) -> Result<()> {
        // Add X-Forwarded-For header
        if let Some(client_addr) = session.client_addr() {
            let client_ip = client_addr.to_string();
            // Remove port from address
            let ip_only = client_ip.split(':').next().unwrap_or(&client_ip);

            // Append to existing X-Forwarded-For or create new
            if let Some(existing) = upstream_request.headers.get("x-forwarded-for") {
                if let Ok(existing_str) = existing.to_str() {
                    let new_value = format!("{}, {}", existing_str, ip_only);
                    let _ = upstream_request.insert_header("X-Forwarded-For", new_value);
                }
            } else {
                let _ = upstream_request.insert_header("X-Forwarded-For", ip_only);
            }

            // Add X-Real-IP (original client only)
            let _ = upstream_request.insert_header("X-Real-IP", ip_only);
        }

        // Add X-Forwarded-Proto
        // TODO: Detect actual protocol from TLS state
        let _ = upstream_request.insert_header("X-Forwarded-Proto", "https");

        // Add X-Forwarded-Host if we have the original host
        if let Some(host) = session.req_header().headers.get("host") {
            if let Ok(host_str) = host.to_str() {
                let _ = upstream_request.insert_header("X-Forwarded-Host", host_str);
            }
        }

        Ok(())
    }

    /// Modify the response before sending to client
    ///
    /// This filter adds:
    /// - Server-Timing header with proxy duration
    /// - X-Served-By header with service name
    fn upstream_response_filter(
        &self,
        _session: &mut Session,
        upstream_response: &mut ResponseHeader,
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        // Add Server-Timing header with proxy duration
        let elapsed = ctx.elapsed();
        let timing_value = format!("proxy;dur={}", elapsed.as_millis());
        let _ = upstream_response.insert_header("Server-Timing", timing_value);

        // Add X-Served-By header if we know the service
        if let Some(ref service_name) = ctx.service_name {
            let _ = upstream_response.insert_header("X-Served-By", service_name.as_str());
        }

        Ok(())
    }

    /// Log the request after completion
    ///
    /// This is called for every request, including failures.
    async fn logging(
        &self,
        session: &mut Session,
        _e: Option<&pingora_core::Error>,
        ctx: &mut Self::CTX,
    ) {
        let status = session
            .response_written()
            .map(|resp| resp.status.as_u16())
            .unwrap_or(0);

        let method = session.req_header().method.as_str();
        let path = session.req_header().uri.path();
        let elapsed = ctx.elapsed();

        let service = ctx.service_name.as_deref().unwrap_or("<unresolved>");
        let backend = ctx.selected_backend.as_deref().unwrap_or("<none>");

        tracing::info!(
            method = %method,
            path = %path,
            status = status,
            service = %service,
            backend = %backend,
            duration_ms = elapsed.as_millis() as u64,
            "Request completed"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ctx_creation() {
        let ctx = ZLayerCtx::new();
        assert!(ctx.service_name.is_none());
        assert!(ctx.selected_backend.is_none());
        // Start time should be recent
        assert!(ctx.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn test_ctx_elapsed() {
        let ctx = ZLayerCtx::new();
        std::thread::sleep(Duration::from_millis(10));
        assert!(ctx.elapsed() >= Duration::from_millis(10));
    }
}
