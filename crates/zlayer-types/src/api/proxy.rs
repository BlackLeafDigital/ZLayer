//! Reverse proxy API DTOs.
//!
//! Wire types for the read-only reverse proxy status endpoints: routes,
//! load-balancer backend groups, TLS certificates, and L4 stream proxies.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

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
