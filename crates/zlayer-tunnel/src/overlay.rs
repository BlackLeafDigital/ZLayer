//! Overlay network integration for tunnel routing
//!
//! Provides traits and types for routing tunnel connections through the
//! overlay network when available, with fallback to direct connections.

use std::net::Ipv4Addr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Routing preference for tunnel connections
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RoutingMode {
    /// Prefer overlay, fall back to direct (default)
    #[default]
    PreferOverlay,
    /// Overlay only (fail if unavailable)
    OverlayOnly,
    /// Direct only (never use overlay, original behavior)
    DirectOnly,
}

/// Overlay connectivity status for a peer
#[derive(Debug, Clone)]
pub enum OverlayReachability {
    /// Reachable via overlay at the given IP
    Reachable(Ipv4Addr),
    /// Not reachable via overlay
    Unreachable,
    /// Overlay not available on this node
    Unavailable,
}

/// Trait for resolving node names to overlay/direct addresses.
///
/// Implemented by the runtime binary, consumed by tunnel code.
pub trait OverlayResolver: Send + Sync {
    /// Resolve a node name to an overlay IP address
    fn resolve_overlay_ip(&self, node_name: &str) -> OverlayReachability;

    /// Resolve a node name to a direct endpoint (e.g. "host:port")
    fn resolve_direct_endpoint(&self, node_name: &str) -> Option<String>;

    /// Get the local node's overlay IP, if overlay is active
    fn local_overlay_ip(&self) -> Option<Ipv4Addr>;

    /// Check whether the overlay network is currently active
    fn overlay_active(&self) -> bool;
}

/// Type-erased overlay resolver
pub type DynOverlayResolver = Arc<dyn OverlayResolver>;

/// Trait for registering tunnel services in overlay DNS.
///
/// Allows tunnel services to be discoverable via the overlay's DNS system.
pub trait TunnelDnsRegistrar: Send + Sync {
    /// Register a service name to an overlay IP and port
    fn register_service(
        &self,
        service_name: &str,
        overlay_ip: Ipv4Addr,
        port: u16,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = std::result::Result<(), String>> + Send + '_>,
    >;

    /// Unregister a previously registered service name
    fn unregister_service(
        &self,
        service_name: &str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = std::result::Result<(), String>> + Send + '_>,
    >;
}

/// Type-erased tunnel DNS registrar
pub type DynTunnelDnsRegistrar = Arc<dyn TunnelDnsRegistrar>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_routing_mode_default() {
        let mode = RoutingMode::default();
        assert_eq!(mode, RoutingMode::PreferOverlay);
    }

    #[test]
    fn test_routing_mode_serde_roundtrip() {
        #[derive(Debug, Deserialize, PartialEq, Eq)]
        struct Wrapper {
            mode: RoutingMode,
        }

        let cases = [
            ("mode = \"preferoverlay\"", RoutingMode::PreferOverlay),
            ("mode = \"overlayonly\"", RoutingMode::OverlayOnly),
            ("mode = \"directonly\"", RoutingMode::DirectOnly),
        ];

        for (toml_str, expected) in &cases {
            let wrapper: Wrapper = toml::from_str(toml_str).expect("deserialize");
            assert_eq!(*expected, wrapper.mode);
        }
    }

    #[test]
    fn test_routing_mode_equality() {
        assert_eq!(RoutingMode::PreferOverlay, RoutingMode::PreferOverlay);
        assert_eq!(RoutingMode::OverlayOnly, RoutingMode::OverlayOnly);
        assert_eq!(RoutingMode::DirectOnly, RoutingMode::DirectOnly);
        assert_ne!(RoutingMode::PreferOverlay, RoutingMode::DirectOnly);
        assert_ne!(RoutingMode::OverlayOnly, RoutingMode::DirectOnly);
    }

    #[test]
    fn test_overlay_reachability_variants() {
        let reachable = OverlayReachability::Reachable(Ipv4Addr::new(10, 0, 0, 1));
        assert!(
            matches!(reachable, OverlayReachability::Reachable(ip) if ip == Ipv4Addr::new(10, 0, 0, 1))
        );

        let unreachable = OverlayReachability::Unreachable;
        assert!(matches!(unreachable, OverlayReachability::Unreachable));

        let unavailable = OverlayReachability::Unavailable;
        assert!(matches!(unavailable, OverlayReachability::Unavailable));
    }
}
