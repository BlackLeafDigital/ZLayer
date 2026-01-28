//! Request routing implementation
//!
//! This module provides routing for matching incoming requests to backend services.

use crate::error::{ProxyError, Result};
use crate::lb::LoadBalancer;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use zlayer_spec::{EndpointSpec, ExposeType, Protocol};

/// A route definition
#[derive(Debug, Clone)]
pub struct Route {
    /// Service name
    pub service: String,
    /// Endpoint name
    pub endpoint: String,
    /// Host pattern (exact or wildcard)
    pub host: Option<String>,
    /// Path prefix
    pub path_prefix: String,
    /// Protocol
    pub protocol: Protocol,
    /// Exposure type
    pub expose: ExposeType,
    /// Strip path prefix before forwarding
    pub strip_prefix: bool,
}

impl Route {
    /// Create a new route from an endpoint spec
    pub fn from_endpoint(service: &str, endpoint: &EndpointSpec) -> Self {
        Self {
            service: service.to_string(),
            endpoint: endpoint.name.clone(),
            host: None,
            path_prefix: endpoint.path.clone().unwrap_or_else(|| "/".to_string()),
            protocol: endpoint.protocol,
            expose: endpoint.expose,
            strip_prefix: false,
        }
    }

    /// Set the host pattern
    pub fn with_host(mut self, host: impl Into<String>) -> Self {
        self.host = Some(host.into());
        self
    }

    /// Set strip prefix behavior
    pub fn with_strip_prefix(mut self, strip: bool) -> Self {
        self.strip_prefix = strip;
        self
    }

    /// Check if this route matches the given host and path
    pub fn matches(&self, host: Option<&str>, path: &str) -> bool {
        // Check host if route specifies one
        if let Some(ref route_host) = self.host {
            match host {
                Some(h) => {
                    if !self.host_matches(route_host, h) {
                        return false;
                    }
                }
                None => return false,
            }
        }

        // Check path prefix
        self.path_matches(path)
    }

    fn host_matches(&self, pattern: &str, host: &str) -> bool {
        if pattern.starts_with("*.") {
            // Wildcard match: *.example.com matches foo.example.com
            let suffix = &pattern[1..]; // .example.com
            host.ends_with(suffix)
        } else {
            // Exact match
            pattern == host
        }
    }

    fn path_matches(&self, path: &str) -> bool {
        if self.path_prefix == "/" {
            return true;
        }

        // Normalize paths for comparison
        let normalized_prefix = self.path_prefix.trim_end_matches('/');
        let normalized_path = path.trim_end_matches('/');

        // Must match prefix exactly or with trailing slash
        normalized_path.starts_with(normalized_prefix)
            && (normalized_path.len() == normalized_prefix.len()
                || (normalized_path.chars().nth(normalized_prefix.len()) == Some('/')))
    }

    /// Transform the path based on route configuration
    pub fn transform_path(&self, path: &str) -> String {
        if !self.strip_prefix || self.path_prefix == "/" {
            return path.to_string();
        }

        let normalized_prefix = self.path_prefix.trim_end_matches('/');
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
}

/// Router for matching requests to services
#[derive(Debug)]
pub struct Router {
    /// Routes ordered by priority (longest prefix first)
    routes: RwLock<Vec<Route>>,
    /// Load balancers per service
    load_balancers: RwLock<HashMap<String, Arc<LoadBalancer>>>,
}

impl Router {
    /// Create a new router
    pub fn new() -> Self {
        Self {
            routes: RwLock::new(Vec::new()),
            load_balancers: RwLock::new(HashMap::new()),
        }
    }

    /// Add a route
    pub async fn add_route(&self, route: Route) {
        let mut routes = self.routes.write().await;

        // Insert maintaining longest-prefix-first order
        let insert_idx = routes
            .iter()
            .position(|r| r.path_prefix.len() < route.path_prefix.len())
            .unwrap_or(routes.len());

        routes.insert(insert_idx, route);
    }

    /// Remove all routes for a service
    pub async fn remove_service_routes(&self, service: &str) {
        let mut routes = self.routes.write().await;
        routes.retain(|r| r.service != service);
    }

    /// Get the load balancer for a service, creating if needed
    pub async fn get_or_create_lb(&self, service: &str) -> Arc<LoadBalancer> {
        let lbs = self.load_balancers.read().await;
        if let Some(lb) = lbs.get(service) {
            return lb.clone();
        }
        drop(lbs);

        let mut lbs = self.load_balancers.write().await;
        // Double-check after acquiring write lock
        if let Some(lb) = lbs.get(service) {
            return lb.clone();
        }

        let lb = Arc::new(LoadBalancer::new(service));
        lbs.insert(service.to_string(), lb.clone());
        lb
    }

    /// Get the load balancer for a service
    pub async fn get_lb(&self, service: &str) -> Option<Arc<LoadBalancer>> {
        self.load_balancers.read().await.get(service).cloned()
    }

    /// Match a request to a route
    pub async fn match_route(&self, host: Option<&str>, path: &str) -> Result<RouteMatch> {
        let routes = self.routes.read().await;

        // Find the first matching route (longest prefix wins due to ordering)
        for route in routes.iter() {
            if route.matches(host, path) {
                let lb = self.get_or_create_lb(&route.service).await;
                return Ok(RouteMatch {
                    route: route.clone(),
                    load_balancer: lb,
                });
            }
        }

        Err(ProxyError::RouteNotFound {
            host: host.unwrap_or("<none>").to_string(),
            path: path.to_string(),
        })
    }

    /// Get all routes (for debugging/admin)
    pub async fn list_routes(&self) -> Vec<Route> {
        self.routes.read().await.clone()
    }

    /// Get route count
    pub async fn route_count(&self) -> usize {
        self.routes.read().await.len()
    }
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of a route match
#[derive(Debug)]
pub struct RouteMatch {
    /// The matched route
    pub route: Route,
    /// Load balancer for the service
    pub load_balancer: Arc<LoadBalancer>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_route_path_matching() {
        let route = Route {
            service: "api".to_string(),
            endpoint: "http".to_string(),
            host: None,
            path_prefix: "/api/v1".to_string(),
            protocol: Protocol::Http,
            expose: ExposeType::Public,
            strip_prefix: false,
        };

        assert!(route.matches(None, "/api/v1"));
        assert!(route.matches(None, "/api/v1/"));
        assert!(route.matches(None, "/api/v1/users"));
        assert!(route.matches(None, "/api/v1/users/123"));
        assert!(!route.matches(None, "/api/v2"));
        assert!(!route.matches(None, "/api"));
        assert!(!route.matches(None, "/"));
    }

    #[test]
    fn test_route_host_matching() {
        let route = Route {
            service: "api".to_string(),
            endpoint: "http".to_string(),
            host: Some("api.example.com".to_string()),
            path_prefix: "/".to_string(),
            protocol: Protocol::Http,
            expose: ExposeType::Public,
            strip_prefix: false,
        };

        assert!(route.matches(Some("api.example.com"), "/anything"));
        assert!(!route.matches(Some("other.example.com"), "/anything"));
        assert!(!route.matches(None, "/anything"));
    }

    #[test]
    fn test_route_wildcard_host() {
        let route = Route {
            service: "api".to_string(),
            endpoint: "http".to_string(),
            host: Some("*.example.com".to_string()),
            path_prefix: "/".to_string(),
            protocol: Protocol::Http,
            expose: ExposeType::Public,
            strip_prefix: false,
        };

        assert!(route.matches(Some("api.example.com"), "/"));
        assert!(route.matches(Some("www.example.com"), "/"));
        assert!(route.matches(Some("foo.example.com"), "/"));
        assert!(!route.matches(Some("example.com"), "/"));
        assert!(!route.matches(Some("other.domain.com"), "/"));
    }

    #[test]
    fn test_route_strip_prefix() {
        let route = Route {
            service: "api".to_string(),
            endpoint: "http".to_string(),
            host: None,
            path_prefix: "/api/v1".to_string(),
            protocol: Protocol::Http,
            expose: ExposeType::Public,
            strip_prefix: true,
        };

        assert_eq!(route.transform_path("/api/v1/users"), "/users");
        assert_eq!(route.transform_path("/api/v1/users/123"), "/users/123");
        assert_eq!(route.transform_path("/api/v1"), "/");
        assert_eq!(route.transform_path("/other"), "/other");
    }

    #[tokio::test]
    async fn test_router_longest_prefix_match() {
        let router = Router::new();

        // Add routes in arbitrary order
        router
            .add_route(Route {
                service: "root".to_string(),
                endpoint: "http".to_string(),
                host: None,
                path_prefix: "/".to_string(),
                protocol: Protocol::Http,
                expose: ExposeType::Public,
                strip_prefix: false,
            })
            .await;

        router
            .add_route(Route {
                service: "api".to_string(),
                endpoint: "http".to_string(),
                host: None,
                path_prefix: "/api".to_string(),
                protocol: Protocol::Http,
                expose: ExposeType::Public,
                strip_prefix: false,
            })
            .await;

        router
            .add_route(Route {
                service: "api-v1".to_string(),
                endpoint: "http".to_string(),
                host: None,
                path_prefix: "/api/v1".to_string(),
                protocol: Protocol::Http,
                expose: ExposeType::Public,
                strip_prefix: false,
            })
            .await;

        // Test longest prefix matching
        let m = router.match_route(None, "/api/v1/users").await.unwrap();
        assert_eq!(m.route.service, "api-v1");

        let m = router.match_route(None, "/api/v2/users").await.unwrap();
        assert_eq!(m.route.service, "api");

        let m = router.match_route(None, "/other").await.unwrap();
        assert_eq!(m.route.service, "root");
    }

    #[tokio::test]
    async fn test_router_no_match() {
        let router = Router::new();

        router
            .add_route(Route {
                service: "api".to_string(),
                endpoint: "http".to_string(),
                host: Some("api.example.com".to_string()),
                path_prefix: "/".to_string(),
                protocol: Protocol::Http,
                expose: ExposeType::Public,
                strip_prefix: false,
            })
            .await;

        let result = router.match_route(Some("other.example.com"), "/").await;
        assert!(matches!(result, Err(ProxyError::RouteNotFound { .. })));
    }
}
