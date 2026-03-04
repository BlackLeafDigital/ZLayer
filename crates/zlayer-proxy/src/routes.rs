//! Service registry for route resolution
//!
//! This module provides a production-ready `ServiceRegistry` for mapping incoming
//! requests to backend services based on host patterns (including wildcards) and
//! path prefixes.  Routes are stored in longest-prefix-first order so that
//! `resolve()` returns the most specific match in O(n).

use std::net::SocketAddr;
use tokio::sync::RwLock;
use zlayer_spec::{EndpointSpec, ExposeType, Protocol};

// ---------------------------------------------------------------------------
// ResolvedService
// ---------------------------------------------------------------------------

/// Fully-resolved service information returned by the registry.
#[derive(Clone, Debug)]
pub struct ResolvedService {
    /// Service name (e.g. "api", "frontend")
    pub name: String,
    /// Backend addresses for load balancing
    pub backends: Vec<SocketAddr>,
    /// Whether to use TLS for upstream connections
    pub use_tls: bool,
    /// SNI hostname for TLS connections
    pub sni_hostname: String,
    /// Exposure type (public / internal)
    pub expose: ExposeType,
    /// Protocol (http, https, tcp, udp, websocket)
    pub protocol: Protocol,
    /// Whether to strip the matched path prefix before forwarding
    pub strip_prefix: bool,
    /// The path prefix this service was registered with
    pub path_prefix: String,
    /// The port the container actually listens on
    pub target_port: u16,
}

// ---------------------------------------------------------------------------
// RouteEntry
// ---------------------------------------------------------------------------

/// A single route entry in the registry.
#[derive(Debug, Clone)]
pub struct RouteEntry {
    /// Owning service name (e.g. "api")
    pub service_name: String,
    /// Endpoint name within that service (e.g. "http", "grpc")
    pub endpoint_name: String,
    /// Host pattern to match.  `None` means match any host.
    /// Supports wildcard patterns like `*.example.com`.
    pub host: Option<String>,
    /// Path prefix to match.  `"/"` matches all paths.
    pub path_prefix: String,
    /// The fully-resolved service returned on match.
    pub resolved: ResolvedService,
}

impl RouteEntry {
    /// Create a `RouteEntry` from a `zlayer_spec::EndpointSpec`.
    ///
    /// Fields that cannot be derived from the spec alone (host, backends, TLS,
    /// SNI) are given sensible defaults and can be overridden after construction.
    #[must_use]
    pub fn from_endpoint(service_name: &str, endpoint: &EndpointSpec) -> Self {
        let path_prefix = endpoint.path.clone().unwrap_or_else(|| "/".to_string());
        let target_port = endpoint.target_port();

        Self {
            service_name: service_name.to_string(),
            endpoint_name: endpoint.name.clone(),
            host: None,
            path_prefix: path_prefix.clone(),
            resolved: ResolvedService {
                name: service_name.to_string(),
                backends: Vec::new(),
                use_tls: endpoint.protocol == Protocol::Https,
                sni_hostname: String::new(),
                expose: endpoint.expose,
                protocol: endpoint.protocol,
                strip_prefix: false,
                path_prefix,
                target_port,
            },
        }
    }

    /// Check whether this route matches the given host and path.
    #[must_use]
    pub fn matches(&self, host: Option<&str>, path: &str) -> bool {
        // If the route specifies a host pattern the request must supply a
        // host that satisfies it.
        if let Some(ref pattern) = self.host {
            match host {
                Some(h) => {
                    if !host_matches(pattern, h) {
                        return false;
                    }
                }
                None => return false,
            }
        }

        path_matches(&self.path_prefix, path)
    }
}

// ---------------------------------------------------------------------------
// ServiceRegistry
// ---------------------------------------------------------------------------

/// Production-ready service registry for the `ZLayer` reverse proxy.
///
/// Routes are stored as a `Vec<RouteEntry>` behind a `tokio::sync::RwLock`,
/// kept in **longest-prefix-first** order so that `resolve()` always returns
/// the most specific match.
pub struct ServiceRegistry {
    /// Routes sorted by descending path-prefix length.
    routes: RwLock<Vec<RouteEntry>>,
}

impl Default for ServiceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            routes: RwLock::new(Vec::new()),
        }
    }

    /// Register a route, maintaining longest-prefix-first order.
    pub async fn register(&self, entry: RouteEntry) {
        let mut routes = self.routes.write().await;

        let insert_idx = routes
            .iter()
            .position(|r| r.path_prefix.len() < entry.path_prefix.len())
            .unwrap_or(routes.len());

        routes.insert(insert_idx, entry);
    }

    /// Remove **all** routes belonging to `service_name`.
    pub async fn unregister_service(&self, service_name: &str) {
        let mut routes = self.routes.write().await;
        routes.retain(|r| r.service_name != service_name);
    }

    /// Resolve an incoming request to the best-matching `ResolvedService`.
    ///
    /// Returns `None` when no route matches.
    pub async fn resolve(&self, host: Option<&str>, path: &str) -> Option<ResolvedService> {
        let routes = self.routes.read().await;

        // First matching route wins (longest prefix is first due to ordering).
        for entry in routes.iter() {
            if entry.matches(host, path) {
                return Some(entry.resolved.clone());
            }
        }

        None
    }

    /// Replace the backend list for every route belonging to `service_name`.
    pub async fn update_backends(&self, service_name: &str, backends: Vec<SocketAddr>) {
        let mut routes = self.routes.write().await;
        for entry in routes.iter_mut() {
            if entry.service_name == service_name {
                entry.resolved.backends = backends.clone();
            }
        }
    }

    /// Append a single backend address to every route belonging to `service_name`.
    pub async fn add_backend(&self, service_name: &str, addr: SocketAddr) {
        let mut routes = self.routes.write().await;
        for entry in routes.iter_mut() {
            if entry.service_name == service_name && !entry.resolved.backends.contains(&addr) {
                entry.resolved.backends.push(addr);
            }
        }
    }

    /// Remove a single backend address from every route belonging to `service_name`.
    pub async fn remove_backend(&self, service_name: &str, addr: SocketAddr) {
        let mut routes = self.routes.write().await;
        for entry in routes.iter_mut() {
            if entry.service_name == service_name {
                entry.resolved.backends.retain(|a| *a != addr);
            }
        }
    }

    /// Return the unique set of service names across all registered routes.
    pub async fn list_services(&self) -> Vec<String> {
        let routes = self.routes.read().await;
        let mut seen = Vec::new();
        for entry in routes.iter() {
            if !seen.contains(&entry.service_name) {
                seen.push(entry.service_name.clone());
            }
        }
        seen
    }

    /// Return the total number of registered routes.
    pub async fn route_count(&self) -> usize {
        self.routes.read().await.len()
    }
}

// ---------------------------------------------------------------------------
// Free functions
// ---------------------------------------------------------------------------

/// Transform `path` by optionally stripping `prefix`.
///
/// When `strip` is `true` the leading `prefix` is removed.  If the result
/// would be empty, `"/"` is returned instead.
#[must_use]
pub fn transform_path(prefix: &str, path: &str, strip: bool) -> String {
    if !strip || prefix == "/" {
        return path.to_string();
    }

    let normalized_prefix = prefix.trim_end_matches('/');
    if let Some(remainder) = path.strip_prefix(normalized_prefix) {
        if remainder.is_empty() {
            "/".to_string()
        } else {
            remainder.to_string()
        }
    } else {
        path.to_string()
    }
}

/// Check whether `pattern` matches `host`.
///
/// Supports simple wildcard patterns: `*.example.com` matches any
/// single-level subdomain such as `api.example.com`.
fn host_matches(pattern: &str, host: &str) -> bool {
    if pattern.starts_with("*.") {
        let suffix = &pattern[1..]; // e.g. ".example.com"
        host.ends_with(suffix)
    } else {
        pattern == host
    }
}

/// Check whether `prefix` matches the beginning of `path` with a proper
/// boundary check (i.e. `/api` matches `/api/foo` but not `/apiary`).
fn path_matches(prefix: &str, path: &str) -> bool {
    if prefix == "/" {
        return true;
    }

    let normalized = prefix.trim_end_matches('/');
    let normalized_path = path.trim_end_matches('/');

    normalized_path.starts_with(normalized)
        && (normalized_path.len() == normalized.len()
            || path.as_bytes().get(normalized.len()) == Some(&b'/'))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Helpers -----------------------------------------------------------

    /// Shorthand to build a minimal `ResolvedService`.
    fn make_resolved(name: &str, backends: Vec<SocketAddr>) -> ResolvedService {
        ResolvedService {
            name: name.to_string(),
            backends,
            use_tls: false,
            sni_hostname: String::new(),
            expose: ExposeType::Internal,
            protocol: Protocol::Http,
            strip_prefix: false,
            path_prefix: "/".to_string(),
            target_port: 8080,
        }
    }

    /// Shorthand to build a `RouteEntry`.
    fn make_entry(
        service: &str,
        host: Option<&str>,
        path: &str,
        backends: Vec<SocketAddr>,
    ) -> RouteEntry {
        let mut resolved = make_resolved(service, backends);
        resolved.path_prefix = path.to_string();
        RouteEntry {
            service_name: service.to_string(),
            endpoint_name: "http".to_string(),
            host: host.map(|s| s.to_string()),
            path_prefix: path.to_string(),
            resolved,
        }
    }

    // -- Ported from routing.rs --------------------------------------------

    #[test]
    fn test_route_path_matching() {
        let entry = make_entry("api", None, "/api/v1", vec![]);

        assert!(entry.matches(None, "/api/v1"));
        assert!(entry.matches(None, "/api/v1/"));
        assert!(entry.matches(None, "/api/v1/users"));
        assert!(entry.matches(None, "/api/v1/users/123"));
        assert!(!entry.matches(None, "/api/v2"));
        assert!(!entry.matches(None, "/api"));
        assert!(!entry.matches(None, "/"));
    }

    #[test]
    fn test_route_host_matching() {
        let entry = make_entry("api", Some("api.example.com"), "/", vec![]);

        assert!(entry.matches(Some("api.example.com"), "/anything"));
        assert!(!entry.matches(Some("other.example.com"), "/anything"));
        assert!(!entry.matches(None, "/anything"));
    }

    #[test]
    fn test_route_wildcard_host() {
        let entry = make_entry("api", Some("*.example.com"), "/", vec![]);

        assert!(entry.matches(Some("api.example.com"), "/"));
        assert!(entry.matches(Some("www.example.com"), "/"));
        assert!(entry.matches(Some("foo.example.com"), "/"));
        assert!(!entry.matches(Some("example.com"), "/"));
        assert!(!entry.matches(Some("other.domain.com"), "/"));
    }

    #[test]
    fn test_route_strip_prefix() {
        assert_eq!(transform_path("/api/v1", "/api/v1/users", true), "/users");
        assert_eq!(
            transform_path("/api/v1", "/api/v1/users/123", true),
            "/users/123"
        );
        assert_eq!(transform_path("/api/v1", "/api/v1", true), "/");
        assert_eq!(transform_path("/api/v1", "/other", true), "/other");
        // strip=false should be a no-op
        assert_eq!(
            transform_path("/api/v1", "/api/v1/users", false),
            "/api/v1/users"
        );
    }

    #[tokio::test]
    async fn test_router_longest_prefix_match() {
        let reg = ServiceRegistry::new();

        reg.register(make_entry("root", None, "/", vec![])).await;
        reg.register(make_entry("api", None, "/api", vec![])).await;
        reg.register(make_entry("api-v1", None, "/api/v1", vec![]))
            .await;

        let m = reg.resolve(None, "/api/v1/users").await.unwrap();
        assert_eq!(m.name, "api-v1");

        let m = reg.resolve(None, "/api/v2/users").await.unwrap();
        assert_eq!(m.name, "api");

        let m = reg.resolve(None, "/other").await.unwrap();
        assert_eq!(m.name, "root");
    }

    #[tokio::test]
    async fn test_router_no_match() {
        let reg = ServiceRegistry::new();

        reg.register(make_entry("api", Some("api.example.com"), "/", vec![]))
            .await;

        let result = reg.resolve(Some("other.example.com"), "/").await;
        assert!(result.is_none());
    }

    // -- New tests ---------------------------------------------------------

    #[tokio::test]
    async fn test_register_and_resolve_host() {
        let reg = ServiceRegistry::new();

        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        reg.register(make_entry("api", Some("api.example.com"), "/", vec![addr]))
            .await;

        let resolved = reg
            .resolve(Some("api.example.com"), "/anything")
            .await
            .unwrap();
        assert_eq!(resolved.name, "api");
        assert_eq!(resolved.backends.len(), 1);
    }

    #[tokio::test]
    async fn test_register_and_resolve_path() {
        let reg = ServiceRegistry::new();

        let addr1: SocketAddr = "127.0.0.1:8081".parse().unwrap();
        let addr2: SocketAddr = "127.0.0.1:8082".parse().unwrap();
        reg.register(make_entry(
            "api-v1",
            Some("api.example.com"),
            "/api/v1",
            vec![addr1],
        ))
        .await;
        reg.register(make_entry(
            "api-v2",
            Some("api.example.com"),
            "/api/v2",
            vec![addr2],
        ))
        .await;

        let resolved = reg
            .resolve(Some("api.example.com"), "/api/v1/users")
            .await
            .unwrap();
        assert_eq!(resolved.name, "api-v1");

        let resolved = reg
            .resolve(Some("api.example.com"), "/api/v2/users")
            .await
            .unwrap();
        assert_eq!(resolved.name, "api-v2");
    }

    #[tokio::test]
    async fn test_resolve_not_found() {
        let reg = ServiceRegistry::new();
        let result = reg.resolve(Some("unknown.example.com"), "/").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_update_backends() {
        let reg = ServiceRegistry::new();

        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        reg.register(make_entry("api", Some("api.example.com"), "/", vec![addr]))
            .await;

        let new_backends: Vec<SocketAddr> = vec![
            "127.0.0.1:8081".parse().unwrap(),
            "127.0.0.1:8082".parse().unwrap(),
        ];
        reg.update_backends("api", new_backends).await;

        let resolved = reg.resolve(Some("api.example.com"), "/").await.unwrap();
        assert_eq!(resolved.backends.len(), 2);
    }

    #[tokio::test]
    async fn test_unregister_service() {
        let reg = ServiceRegistry::new();

        reg.register(make_entry("api", None, "/api", vec![])).await;
        reg.register(make_entry("web", None, "/", vec![])).await;

        assert_eq!(reg.route_count().await, 2);
        reg.unregister_service("api").await;
        assert_eq!(reg.route_count().await, 1);

        // "api" route should be gone
        let result = reg.resolve(None, "/api/foo").await;
        // The "/" route still matches /api/foo so it resolves to "web"
        assert_eq!(result.unwrap().name, "web");
    }

    #[tokio::test]
    async fn test_list_services() {
        let reg = ServiceRegistry::new();

        reg.register(make_entry("api", None, "/api", vec![])).await;
        reg.register(make_entry("api", None, "/api/v2", vec![]))
            .await;
        reg.register(make_entry("web", None, "/", vec![])).await;

        let mut services = reg.list_services().await;
        services.sort();
        assert_eq!(services, vec!["api", "web"]);
    }

    #[tokio::test]
    async fn test_route_count() {
        let reg = ServiceRegistry::new();
        assert_eq!(reg.route_count().await, 0);

        reg.register(make_entry("a", None, "/a", vec![])).await;
        reg.register(make_entry("b", None, "/b", vec![])).await;
        reg.register(make_entry("c", None, "/c", vec![])).await;
        assert_eq!(reg.route_count().await, 3);
    }

    #[tokio::test]
    async fn test_add_remove_backend() {
        let reg = ServiceRegistry::new();

        let b1: SocketAddr = "127.0.0.1:8001".parse().unwrap();
        reg.register(make_entry("api", None, "/", vec![b1])).await;

        let b2: SocketAddr = "127.0.0.1:8002".parse().unwrap();
        reg.add_backend("api", b2).await;

        let resolved = reg.resolve(None, "/").await.unwrap();
        assert_eq!(resolved.backends.len(), 2);
        assert!(resolved.backends.contains(&b1));
        assert!(resolved.backends.contains(&b2));

        // Adding a duplicate should not create a second entry
        reg.add_backend("api", b2).await;
        let resolved = reg.resolve(None, "/").await.unwrap();
        assert_eq!(resolved.backends.len(), 2);

        // Remove b1
        reg.remove_backend("api", b1).await;
        let resolved = reg.resolve(None, "/").await.unwrap();
        assert_eq!(resolved.backends.len(), 1);
        assert_eq!(resolved.backends[0], b2);
    }

    #[tokio::test]
    async fn test_from_endpoint() {
        let endpoint = EndpointSpec {
            name: "http".to_string(),
            protocol: Protocol::Http,
            port: 80,
            target_port: Some(8080),
            path: Some("/api".to_string()),
            expose: ExposeType::Public,
            stream: None,
            tunnel: None,
        };

        let entry = RouteEntry::from_endpoint("my-service", &endpoint);
        assert_eq!(entry.service_name, "my-service");
        assert_eq!(entry.endpoint_name, "http");
        assert!(entry.host.is_none());
        assert_eq!(entry.path_prefix, "/api");
        assert_eq!(entry.resolved.name, "my-service");
        assert_eq!(entry.resolved.protocol, Protocol::Http);
        assert_eq!(entry.resolved.expose, ExposeType::Public);
        assert_eq!(entry.resolved.target_port, 8080);
        assert!(!entry.resolved.use_tls);
        assert!(entry.resolved.backends.is_empty());
    }
}
