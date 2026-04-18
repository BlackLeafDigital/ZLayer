//! Reverse proxy status endpoints
//!
//! Read-only endpoints for inspecting the state of the `ZLayer` reverse proxy:
//! routes, load-balancer backend groups, TLS certificates, and L4 stream
//! proxies.

use std::sync::Arc;

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::{ApiError, Result};
use zlayer_proxy::{CertManager, LoadBalancer, ServiceRegistry, StreamRegistry};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Shared state for proxy status endpoints.
///
/// Each field is optional so the API can still serve partial information when
/// a subsystem was not initialised (e.g. TLS disabled, no L4 streams).
#[derive(Clone)]
pub struct ProxyApiState {
    /// HTTP/HTTPS route registry (L7).
    pub registry: Option<Arc<ServiceRegistry>>,
    /// Load balancer with per-service backend groups.
    pub load_balancer: Option<Arc<LoadBalancer>>,
    /// TLS certificate manager (ACME / manual).
    pub cert_manager: Option<Arc<CertManager>>,
    /// L4 TCP/UDP stream registry.
    pub stream_registry: Option<Arc<StreamRegistry>>,
}

impl Default for ProxyApiState {
    fn default() -> Self {
        Self::new()
    }
}

impl ProxyApiState {
    /// Create an empty state (all subsystems disabled).
    #[must_use]
    pub fn new() -> Self {
        Self {
            registry: None,
            load_balancer: None,
            cert_manager: None,
            stream_registry: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Information about a single registered route.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RouteInfo {
    /// Owning service name.
    pub service: String,
    /// Endpoint name within the service.
    pub endpoint: String,
    /// Host pattern (e.g. `*.example.com`), or `null` for any host.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Path prefix matched by this route.
    pub path_prefix: String,
    /// Whether the matched prefix is stripped before forwarding.
    pub strip_prefix: bool,
    /// Protocol (http, https, tcp, etc.).
    pub protocol: String,
    /// Exposure type (public / internal).
    pub expose: String,
    /// Backend addresses currently assigned to this route.
    pub backends: Vec<String>,
    /// Container target port.
    pub target_port: u16,
}

/// Response for `GET /api/v1/proxy/routes`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RoutesResponse {
    /// Total number of routes.
    pub total: usize,
    /// Route details.
    pub routes: Vec<RouteInfo>,
}

/// Information about a single backend in a load-balancer group.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BackendInfo {
    /// Backend address (`ip:port`).
    pub address: String,
    /// Whether the backend is currently healthy.
    pub healthy: bool,
    /// Number of in-flight connections.
    pub active_connections: u64,
    /// Number of consecutive health-check failures.
    pub consecutive_failures: u64,
}

/// A load-balancer backend group for one service.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BackendGroupInfo {
    /// Service name.
    pub service: String,
    /// Load-balancing strategy (`round_robin` or `least_connections`).
    pub strategy: String,
    /// Backends in this group.
    pub backends: Vec<BackendInfo>,
    /// Number of healthy backends.
    pub healthy_count: usize,
    /// Total number of backends.
    pub total_count: usize,
}

/// Response for `GET /api/v1/proxy/backends`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BackendsResponse {
    /// Total number of backend groups.
    pub total_groups: usize,
    /// Backend group details.
    pub groups: Vec<BackendGroupInfo>,
}

/// Information about a loaded TLS certificate.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CertInfo {
    /// Domain the certificate covers.
    pub domain: String,
    /// Certificate validity start (ISO-8601), if metadata is available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_before: Option<String>,
    /// Certificate expiry (ISO-8601), if metadata is available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_after: Option<String>,
    /// SHA-256 fingerprint, if metadata is available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    /// Whether the certificate needs renewal (within 30 days of expiry).
    pub needs_renewal: bool,
}

/// Response for `GET /api/v1/proxy/tls`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TlsResponse {
    /// Total number of cached certificates.
    pub total: usize,
    /// ACME email address, if configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acme_email: Option<String>,
    /// Whether ACME auto-provisioning is available.
    pub acme_available: bool,
    /// Certificate details.
    pub certificates: Vec<CertInfo>,
}

/// Information about a single L4 stream proxy backend.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct StreamBackendInfo {
    /// Backend address (`ip:port`).
    pub address: String,
}

/// Information about a single L4 stream proxy.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct StreamInfo {
    /// Listen port on this node.
    pub port: u16,
    /// Transport protocol (`tcp` or `udp`).
    pub protocol: String,
    /// Service name.
    pub service: String,
    /// Number of backends.
    pub backend_count: usize,
    /// Backend addresses.
    pub backends: Vec<StreamBackendInfo>,
}

/// Response for `GET /api/v1/proxy/streams`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct StreamsResponse {
    /// Total number of stream proxies.
    pub total: usize,
    /// TCP stream count.
    pub tcp_count: usize,
    /// UDP stream count.
    pub udp_count: usize,
    /// Stream details.
    pub streams: Vec<StreamInfo>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// List all registered proxy routes.
///
/// Returns the full list of L7 routes with their host patterns, path prefixes,
/// backends, and protocol information.
///
/// # Errors
///
/// Returns `ApiError::ServiceUnavailable` if the service registry is not initialised.
#[utoipa::path(
    get,
    path = "/api/v1/proxy/routes",
    responses(
        (status = 200, description = "List of routes", body = RoutesResponse),
        (status = 503, description = "Service registry not available"),
    ),
    security(("bearer_auth" = [])),
    tag = "Proxy"
)]
pub async fn list_routes(State(state): State<ProxyApiState>) -> Result<Json<RoutesResponse>> {
    let registry = state.registry.as_ref().ok_or(ApiError::ServiceUnavailable(
        "Service registry not available".to_string(),
    ))?;

    let entries = registry.list_routes().await;
    let routes: Vec<RouteInfo> = entries
        .iter()
        .map(|e| RouteInfo {
            service: e.service_name.clone(),
            endpoint: e.endpoint_name.clone(),
            host: e.host.clone(),
            path_prefix: e.path_prefix.clone(),
            strip_prefix: e.resolved.strip_prefix,
            protocol: format!("{:?}", e.resolved.protocol).to_lowercase(),
            expose: format!("{:?}", e.resolved.expose).to_lowercase(),
            backends: e
                .resolved
                .backends
                .iter()
                .map(std::string::ToString::to_string)
                .collect(),
            target_port: e.resolved.target_port,
        })
        .collect();

    Ok(Json(RoutesResponse {
        total: routes.len(),
        routes,
    }))
}

/// List all load-balancer backend groups.
///
/// Returns each service's backend group with its strategy, health status, and
/// active connection counts.
///
/// # Errors
///
/// Returns `ApiError::ServiceUnavailable` if the load balancer is not initialised.
#[utoipa::path(
    get,
    path = "/api/v1/proxy/backends",
    responses(
        (status = 200, description = "Load-balancer backend groups", body = BackendsResponse),
        (status = 503, description = "Load balancer not available"),
    ),
    security(("bearer_auth" = [])),
    tag = "Proxy"
)]
pub async fn list_backends(State(state): State<ProxyApiState>) -> Result<Json<BackendsResponse>> {
    let lb = state
        .load_balancer
        .as_ref()
        .ok_or(ApiError::ServiceUnavailable(
            "Load balancer not available".to_string(),
        ))?;

    let service_names = lb.list_service_names();
    let mut groups = Vec::with_capacity(service_names.len());

    for name in &service_names {
        if let Some(snapshot) = lb.group_snapshot(name) {
            let healthy_count = snapshot.backends.iter().filter(|b| b.healthy).count();
            let total_count = snapshot.backends.len();

            let strategy_str = match snapshot.strategy {
                zlayer_proxy::LbStrategy::RoundRobin => "round_robin",
                zlayer_proxy::LbStrategy::LeastConnections => "least_connections",
            };

            groups.push(BackendGroupInfo {
                service: name.clone(),
                strategy: strategy_str.to_string(),
                backends: snapshot
                    .backends
                    .iter()
                    .map(|b| BackendInfo {
                        address: b.addr.to_string(),
                        healthy: b.healthy,
                        active_connections: b.active_connections,
                        consecutive_failures: b.consecutive_failures,
                    })
                    .collect(),
                healthy_count,
                total_count,
            });
        }
    }

    Ok(Json(BackendsResponse {
        total_groups: groups.len(),
        groups,
    }))
}

/// List loaded TLS certificates.
///
/// Returns cached certificate domains with metadata (expiry, fingerprint)
/// when available.
///
/// # Errors
///
/// Returns `ApiError::ServiceUnavailable` if the certificate manager is not initialised.
#[utoipa::path(
    get,
    path = "/api/v1/proxy/tls",
    responses(
        (status = 200, description = "Loaded TLS certificates", body = TlsResponse),
        (status = 503, description = "Certificate manager not available"),
    ),
    security(("bearer_auth" = [])),
    tag = "Proxy"
)]
pub async fn list_tls(State(state): State<ProxyApiState>) -> Result<Json<TlsResponse>> {
    let cm = state
        .cert_manager
        .as_ref()
        .ok_or(ApiError::ServiceUnavailable(
            "Certificate manager not available".to_string(),
        ))?;

    let domains = cm.list_cached_domains().await;
    let renewal_domains = cm.get_domains_needing_renewal().await;
    let acme_email = cm.acme_email().map(String::from);

    let mut certificates = Vec::with_capacity(domains.len());
    for domain in &domains {
        let meta = cm.load_cert_metadata(domain).await;
        let needs_renewal = renewal_domains.contains(domain);

        certificates.push(CertInfo {
            domain: domain.clone(),
            not_before: meta.as_ref().map(|m| m.not_before.to_rfc3339()),
            not_after: meta.as_ref().map(|m| m.not_after.to_rfc3339()),
            fingerprint: meta.as_ref().map(|m| m.fingerprint.clone()),
            needs_renewal,
        });
    }

    Ok(Json(TlsResponse {
        total: certificates.len(),
        acme_email,
        acme_available: true,
        certificates,
    }))
}

/// List L4 stream proxies.
///
/// Returns all TCP and UDP stream proxies with their listen ports, service
/// names, and backends.
///
/// # Errors
///
/// Returns `ApiError::ServiceUnavailable` if the stream registry is not initialised.
#[utoipa::path(
    get,
    path = "/api/v1/proxy/streams",
    responses(
        (status = 200, description = "L4 stream proxies", body = StreamsResponse),
        (status = 503, description = "Stream registry not available"),
    ),
    security(("bearer_auth" = [])),
    tag = "Proxy"
)]
pub async fn list_streams(State(state): State<ProxyApiState>) -> Result<Json<StreamsResponse>> {
    let sr = state
        .stream_registry
        .as_ref()
        .ok_or(ApiError::ServiceUnavailable(
            "Stream registry not available".to_string(),
        ))?;

    let tcp_services = sr.list_tcp_services();
    let udp_services = sr.list_udp_services();

    let mut streams = Vec::with_capacity(tcp_services.len() + udp_services.len());

    for (port, svc) in &tcp_services {
        streams.push(StreamInfo {
            port: *port,
            protocol: "tcp".to_string(),
            service: svc.name.clone(),
            backend_count: svc.backend_count(),
            backends: svc
                .backends
                .iter()
                .map(|b| StreamBackendInfo {
                    address: b.to_string(),
                })
                .collect(),
        });
    }

    for (port, svc) in &udp_services {
        streams.push(StreamInfo {
            port: *port,
            protocol: "udp".to_string(),
            service: svc.name.clone(),
            backend_count: svc.backend_count(),
            backends: svc
                .backends
                .iter()
                .map(|b| StreamBackendInfo {
                    address: b.to_string(),
                })
                .collect(),
        });
    }

    Ok(Json(StreamsResponse {
        total: streams.len(),
        tcp_count: tcp_services.len(),
        udp_count: udp_services.len(),
        streams,
    }))
}
