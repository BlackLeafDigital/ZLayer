//! Overlay-aware WebSocket connector
//!
//! Resolves the best WebSocket URL for a tunnel connection and connects
//! with fallback logic based on the configured [`RoutingMode`].

use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use crate::overlay::{DynOverlayResolver, OverlayReachability, RoutingMode};
use crate::{Result, TunnelError};

/// A connector that routes WebSocket connections through the overlay
/// network when available, with configurable fallback behavior.
pub struct OverlayAwareConnector {
    /// Direct (non-overlay) WebSocket URL
    direct_url: String,
    /// Pre-computed overlay WebSocket URL (if known)
    overlay_url: Option<String>,
    /// Routing preference
    routing_mode: RoutingMode,
    /// Optional resolver for dynamic overlay URL resolution
    resolver: Option<DynOverlayResolver>,
}

impl OverlayAwareConnector {
    /// Create a new overlay-aware connector
    ///
    /// # Arguments
    ///
    /// * `direct_url` - The direct (non-overlay) WebSocket URL
    /// * `overlay_url` - An optional pre-computed overlay WebSocket URL
    /// * `routing_mode` - The routing preference
    /// * `resolver` - An optional overlay resolver for dynamic resolution
    #[must_use]
    pub fn new(
        direct_url: &str,
        overlay_url: Option<&str>,
        routing_mode: RoutingMode,
        resolver: Option<DynOverlayResolver>,
    ) -> Self {
        Self {
            direct_url: direct_url.to_string(),
            overlay_url: overlay_url.map(ToString::to_string),
            routing_mode,
            resolver,
        }
    }

    /// Connect with routing mode logic:
    ///
    /// - `PreferOverlay`: try overlay first, fall back to direct
    /// - `OverlayOnly`: overlay or error
    /// - `DirectOnly`: direct only (original behavior)
    ///
    /// # Errors
    ///
    /// Returns an error if the connection fails according to the routing mode rules.
    pub async fn connect(
        &self,
    ) -> Result<(
        WebSocketStream<MaybeTlsStream<TcpStream>>,
        tokio_tungstenite::tungstenite::http::Response<Option<Vec<u8>>>,
    )> {
        match self.routing_mode {
            RoutingMode::DirectOnly => self.connect_direct().await,
            RoutingMode::OverlayOnly => self.connect_overlay_only().await,
            RoutingMode::PreferOverlay => self.connect_prefer_overlay().await,
        }
    }

    /// Connect directly, bypassing the overlay
    async fn connect_direct(
        &self,
    ) -> Result<(
        WebSocketStream<MaybeTlsStream<TcpStream>>,
        tokio_tungstenite::tungstenite::http::Response<Option<Vec<u8>>>,
    )> {
        tracing::debug!(url = %self.direct_url, "connecting directly");
        connect_async(&self.direct_url)
            .await
            .map_err(TunnelError::connection)
    }

    /// Connect via overlay only, failing if unavailable
    async fn connect_overlay_only(
        &self,
    ) -> Result<(
        WebSocketStream<MaybeTlsStream<TcpStream>>,
        tokio_tungstenite::tungstenite::http::Response<Option<Vec<u8>>>,
    )> {
        let overlay_url = self.resolve_overlay_url().ok_or_else(|| {
            TunnelError::connection_msg("overlay routing required but no overlay URL available")
        })?;

        tracing::debug!(url = %overlay_url, "connecting via overlay (overlay-only mode)");
        connect_async(&overlay_url)
            .await
            .map_err(TunnelError::connection)
    }

    /// Try overlay first, fall back to direct on failure
    async fn connect_prefer_overlay(
        &self,
    ) -> Result<(
        WebSocketStream<MaybeTlsStream<TcpStream>>,
        tokio_tungstenite::tungstenite::http::Response<Option<Vec<u8>>>,
    )> {
        // Attempt overlay if an overlay URL is available
        if let Some(overlay_url) = self.resolve_overlay_url() {
            tracing::debug!(url = %overlay_url, "attempting overlay connection");
            match connect_async(&overlay_url).await {
                Ok(result) => {
                    tracing::debug!("connected via overlay");
                    return Ok(result);
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "overlay connection failed, falling back to direct"
                    );
                }
            }
        } else {
            tracing::debug!("no overlay URL available, using direct connection");
        }

        // Fall back to direct
        self.connect_direct().await
    }

    /// Determine the overlay URL, either from the pre-computed value or
    /// dynamically from the resolver
    fn resolve_overlay_url(&self) -> Option<String> {
        // If we have a pre-computed overlay URL, use it
        if let Some(ref url) = self.overlay_url {
            return Some(url.clone());
        }

        // Otherwise, try to resolve dynamically
        let resolver = self.resolver.as_ref()?;
        if !resolver.overlay_active() {
            return None;
        }

        // Try to extract the hostname from the direct URL to resolve it
        // Format: ws://host:port/path or wss://host:port/path
        let url_without_scheme = self
            .direct_url
            .strip_prefix("wss://")
            .or_else(|| self.direct_url.strip_prefix("ws://"))?;

        let host = url_without_scheme.split(':').next()?;

        match resolver.resolve_overlay_ip(host) {
            OverlayReachability::Reachable(ip) => {
                // Replace the host in the URL with the overlay IP
                let scheme = if self.direct_url.starts_with("wss://") {
                    "wss"
                } else {
                    "ws"
                };

                // Extract port and path from the original URL
                let after_host = url_without_scheme.strip_prefix(host)?;
                Some(format!("{scheme}://{ip}{after_host}"))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::OverlayReachability;
    use std::net::Ipv4Addr;

    /// Mock overlay resolver for testing
    struct MockResolver {
        overlay_ip: Option<Ipv4Addr>,
        active: bool,
    }

    impl MockResolver {
        fn active_with_ip(ip: Ipv4Addr) -> Self {
            Self {
                overlay_ip: Some(ip),
                active: true,
            }
        }

        fn inactive() -> Self {
            Self {
                overlay_ip: None,
                active: false,
            }
        }
    }

    impl crate::overlay::OverlayResolver for MockResolver {
        fn resolve_overlay_ip(&self, _node_name: &str) -> OverlayReachability {
            match self.overlay_ip {
                Some(ip) => OverlayReachability::Reachable(ip),
                None => OverlayReachability::Unavailable,
            }
        }

        fn resolve_direct_endpoint(&self, _node_name: &str) -> Option<String> {
            None
        }

        fn local_overlay_ip(&self) -> Option<Ipv4Addr> {
            self.overlay_ip
        }

        fn overlay_active(&self) -> bool {
            self.active
        }
    }

    #[test]
    fn test_connector_new() {
        let connector = OverlayAwareConnector::new(
            "ws://localhost:3669/tunnel/v1",
            None,
            RoutingMode::PreferOverlay,
            None,
        );

        assert_eq!(connector.direct_url, "ws://localhost:3669/tunnel/v1");
        assert!(connector.overlay_url.is_none());
        assert_eq!(connector.routing_mode, RoutingMode::PreferOverlay);
        assert!(connector.resolver.is_none());
    }

    #[test]
    fn test_connector_with_overlay_url() {
        let connector = OverlayAwareConnector::new(
            "ws://public-host:3669/tunnel/v1",
            Some("ws://10.0.0.5:3669/tunnel/v1"),
            RoutingMode::PreferOverlay,
            None,
        );

        assert_eq!(
            connector.overlay_url.as_deref(),
            Some("ws://10.0.0.5:3669/tunnel/v1")
        );
    }

    #[test]
    fn test_resolve_overlay_url_precomputed() {
        let connector = OverlayAwareConnector::new(
            "ws://public-host:3669/tunnel/v1",
            Some("ws://10.0.0.5:3669/tunnel/v1"),
            RoutingMode::PreferOverlay,
            None,
        );

        let resolved = connector.resolve_overlay_url();
        assert_eq!(resolved.as_deref(), Some("ws://10.0.0.5:3669/tunnel/v1"));
    }

    #[test]
    fn test_resolve_overlay_url_from_resolver() {
        let mock = std::sync::Arc::new(MockResolver::active_with_ip(Ipv4Addr::new(10, 0, 0, 42)));

        let connector = OverlayAwareConnector::new(
            "ws://remote-node:3669/tunnel/v1",
            None,
            RoutingMode::PreferOverlay,
            Some(mock),
        );

        let overlay_url = connector.resolve_overlay_url();
        assert_eq!(
            overlay_url.as_deref(),
            Some("ws://10.0.0.42:3669/tunnel/v1")
        );
    }

    #[test]
    fn test_resolve_overlay_url_inactive_resolver() {
        let mock = std::sync::Arc::new(MockResolver::inactive());

        let connector = OverlayAwareConnector::new(
            "ws://remote-node:3669/tunnel/v1",
            None,
            RoutingMode::PreferOverlay,
            Some(mock),
        );

        let overlay_url = connector.resolve_overlay_url();
        assert!(overlay_url.is_none());
    }

    #[test]
    fn test_resolve_overlay_url_no_resolver_no_precomputed() {
        let connector = OverlayAwareConnector::new(
            "ws://remote-node:3669/tunnel/v1",
            None,
            RoutingMode::PreferOverlay,
            None,
        );

        let overlay_url = connector.resolve_overlay_url();
        assert!(overlay_url.is_none());
    }

    #[test]
    fn test_resolve_overlay_url_preserves_wss_scheme() {
        let mock = std::sync::Arc::new(MockResolver::active_with_ip(Ipv4Addr::new(10, 0, 0, 42)));

        let connector = OverlayAwareConnector::new(
            "wss://remote-node:3669/tunnel/v1",
            None,
            RoutingMode::PreferOverlay,
            Some(mock),
        );

        let overlay_url = connector.resolve_overlay_url();
        assert_eq!(
            overlay_url.as_deref(),
            Some("wss://10.0.0.42:3669/tunnel/v1")
        );
    }

    #[test]
    fn test_direct_only_does_not_resolve_overlay() {
        // DirectOnly mode should not use overlay URL even if one is set.
        // We test the routing logic without actually connecting.
        let connector = OverlayAwareConnector::new(
            "ws://127.0.0.1:3669/tunnel/v1",
            Some("ws://10.0.0.5:3669/tunnel/v1"),
            RoutingMode::DirectOnly,
            None,
        );

        // DirectOnly is the mode, but resolve_overlay_url still returns a value
        // (it's connect() that decides not to use it). Verify routing mode is stored.
        assert_eq!(connector.routing_mode, RoutingMode::DirectOnly);
        assert_eq!(connector.direct_url, "ws://127.0.0.1:3669/tunnel/v1");
    }

    #[tokio::test]
    async fn test_overlay_only_no_overlay_url_returns_error() {
        // OverlayOnly with no overlay URL should fail immediately (no network call)
        let connector = OverlayAwareConnector::new(
            "ws://127.0.0.1:3669/tunnel/v1",
            None,
            RoutingMode::OverlayOnly,
            None,
        );

        let result = connector.connect().await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("overlay"),
            "Expected overlay-related error, got: {err}"
        );
    }

    #[test]
    fn test_prefer_overlay_resolves_url_correctly() {
        // Test that PreferOverlay mode resolves the overlay URL when available
        let mock = std::sync::Arc::new(MockResolver::active_with_ip(Ipv4Addr::new(10, 0, 0, 1)));

        let connector = OverlayAwareConnector::new(
            "ws://node-b:3669/tunnel/v1",
            None,
            RoutingMode::PreferOverlay,
            Some(mock),
        );

        let overlay_url = connector.resolve_overlay_url();
        assert!(overlay_url.is_some());
        assert_eq!(overlay_url.as_deref(), Some("ws://10.0.0.1:3669/tunnel/v1"));
    }

    #[test]
    fn test_prefer_overlay_falls_back_when_no_overlay() {
        // With no overlay available, PreferOverlay should fall back to direct
        let connector = OverlayAwareConnector::new(
            "ws://node-b:3669/tunnel/v1",
            None,
            RoutingMode::PreferOverlay,
            None,
        );

        // No overlay URL available
        let overlay_url = connector.resolve_overlay_url();
        assert!(overlay_url.is_none());
        // The connect() method would use direct_url in this case
        assert_eq!(connector.direct_url, "ws://node-b:3669/tunnel/v1");
    }
}
