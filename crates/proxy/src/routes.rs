//! Service registry for route resolution
//!
//! This module provides the ServiceRegistry for mapping incoming requests
//! to backend services based on host and path patterns.
//!
//! NOTE: This is a minimal implementation for Phase 2 (ZLayerProxy).
//! Full implementation will be completed in Phase 3.

use dashmap::DashMap;
use std::net::SocketAddr;

/// Resolved service information returned by the registry
#[derive(Clone, Debug)]
pub struct ResolvedService {
    /// Service name
    pub name: String,
    /// Backend addresses for load balancing
    pub backends: Vec<SocketAddr>,
    /// Whether to use TLS for upstream connections
    pub use_tls: bool,
    /// SNI hostname for TLS connections
    pub sni_hostname: String,
}

/// Service registry for routing requests to backends
///
/// The registry maintains mappings from:
/// - Host patterns to services
/// - Host + path patterns to services
pub struct ServiceRegistry {
    /// Host-only routes (host -> service)
    host_routes: DashMap<String, ResolvedService>,
    /// Path-specific routes ((host, path_prefix) -> service)
    path_routes: DashMap<(String, String), ResolvedService>,
}

impl Default for ServiceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceRegistry {
    /// Create a new empty service registry
    pub fn new() -> Self {
        Self {
            host_routes: DashMap::new(),
            path_routes: DashMap::new(),
        }
    }

    /// Register a service for a host and optional path
    ///
    /// # Arguments
    /// * `host` - The hostname to match (e.g., "api.example.com")
    /// * `path` - Optional path prefix (e.g., "/api/v1")
    /// * `service` - The resolved service information
    pub fn register(&self, host: &str, path: Option<&str>, service: ResolvedService) {
        match path {
            Some(p) => {
                self.path_routes
                    .insert((host.to_string(), p.to_string()), service);
            }
            None => {
                self.host_routes.insert(host.to_string(), service);
            }
        }
    }

    /// Unregister all routes for a service
    pub fn unregister_service(&self, service_name: &str) {
        self.host_routes.retain(|_, v| v.name != service_name);
        self.path_routes.retain(|_, v| v.name != service_name);
    }

    /// Resolve a request to a service
    ///
    /// # Arguments
    /// * `host` - The request host
    /// * `path` - The request path
    ///
    /// # Returns
    /// The resolved service or an error if no matching route is found
    pub fn resolve(&self, host: &str, path: &str) -> Result<ResolvedService, String> {
        // Try path-specific routes first (more specific)
        for entry in self.path_routes.iter() {
            let (route_host, route_path) = entry.key();
            if host == route_host && path.starts_with(route_path) {
                return Ok(entry.value().clone());
            }
        }

        // Fall back to host-only routes
        self.host_routes
            .get(host)
            .map(|s| s.clone())
            .ok_or_else(|| format!("No service found for host: {}", host))
    }

    /// Update backends for an existing service
    ///
    /// This updates backends in all routes that reference the service.
    pub fn update_backends(&self, service_name: &str, backends: Vec<SocketAddr>) {
        // Update host routes
        for mut entry in self.host_routes.iter_mut() {
            if entry.value().name == service_name {
                entry.value_mut().backends = backends.clone();
            }
        }

        // Update path routes
        for mut entry in self.path_routes.iter_mut() {
            if entry.value().name == service_name {
                entry.value_mut().backends = backends.clone();
            }
        }
    }

    /// Get all registered services (for debugging/admin)
    pub fn list_services(&self) -> Vec<String> {
        let mut services: Vec<String> = self
            .host_routes
            .iter()
            .map(|e| e.value().name.clone())
            .collect();

        for entry in self.path_routes.iter() {
            let name = entry.value().name.clone();
            if !services.contains(&name) {
                services.push(name);
            }
        }

        services
    }

    /// Get route count
    pub fn route_count(&self) -> usize {
        self.host_routes.len() + self.path_routes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_resolve_host() {
        let registry = ServiceRegistry::new();

        let service = ResolvedService {
            name: "api".to_string(),
            backends: vec!["127.0.0.1:8080".parse().unwrap()],
            use_tls: false,
            sni_hostname: "api.local".to_string(),
        };

        registry.register("api.example.com", None, service);

        let resolved = registry.resolve("api.example.com", "/anything").unwrap();
        assert_eq!(resolved.name, "api");
        assert_eq!(resolved.backends.len(), 1);
    }

    #[test]
    fn test_register_and_resolve_path() {
        let registry = ServiceRegistry::new();

        let api_v1 = ResolvedService {
            name: "api-v1".to_string(),
            backends: vec!["127.0.0.1:8081".parse().unwrap()],
            use_tls: false,
            sni_hostname: "".to_string(),
        };

        let api_v2 = ResolvedService {
            name: "api-v2".to_string(),
            backends: vec!["127.0.0.1:8082".parse().unwrap()],
            use_tls: false,
            sni_hostname: "".to_string(),
        };

        registry.register("api.example.com", Some("/api/v1"), api_v1);
        registry.register("api.example.com", Some("/api/v2"), api_v2);

        let resolved = registry
            .resolve("api.example.com", "/api/v1/users")
            .unwrap();
        assert_eq!(resolved.name, "api-v1");

        let resolved = registry
            .resolve("api.example.com", "/api/v2/users")
            .unwrap();
        assert_eq!(resolved.name, "api-v2");
    }

    #[test]
    fn test_resolve_not_found() {
        let registry = ServiceRegistry::new();
        let result = registry.resolve("unknown.example.com", "/");
        assert!(result.is_err());
    }

    #[test]
    fn test_update_backends() {
        let registry = ServiceRegistry::new();

        let service = ResolvedService {
            name: "api".to_string(),
            backends: vec!["127.0.0.1:8080".parse().unwrap()],
            use_tls: false,
            sni_hostname: "".to_string(),
        };

        registry.register("api.example.com", None, service);

        // Update backends
        registry.update_backends(
            "api",
            vec![
                "127.0.0.1:8081".parse().unwrap(),
                "127.0.0.1:8082".parse().unwrap(),
            ],
        );

        let resolved = registry.resolve("api.example.com", "/").unwrap();
        assert_eq!(resolved.backends.len(), 2);
    }
}
