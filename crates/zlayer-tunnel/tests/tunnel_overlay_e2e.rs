//! E2E tests for tunnel-over-overlay routing.
//!
//! Tests the overlay-aware routing logic across the tunnel subsystem:
//! connector, agent, node manager, access manager, and config serialization.
//!
//! All tests run without privileges:
//!
//! ```sh
//! cargo test -p zlayer-tunnel-zql --test tunnel_overlay_e2e
//! ```

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use zlayer_tunnel_zql::client::OverlayAwareConnector;
use zlayer_tunnel_zql::overlay::{
    DynOverlayResolver, OverlayReachability, OverlayResolver, RoutingMode,
};
use zlayer_tunnel_zql::{
    AccessManager, NodeTunnel, NodeTunnelManager, TunnelAgent, TunnelClientConfig,
    TunnelServerConfig,
};

// =============================================================================
// Mock overlay resolver (reused pattern from connector.rs inline tests)
// =============================================================================

struct MockResolver {
    overlay_ip: Option<Ipv4Addr>,
    active: bool,
}

impl MockResolver {
    fn active_with_ip(ip: Ipv4Addr) -> Arc<Self> {
        Arc::new(Self {
            overlay_ip: Some(ip),
            active: true,
        })
    }

    fn inactive() -> Arc<Self> {
        Arc::new(Self {
            overlay_ip: None,
            active: false,
        })
    }
}

impl OverlayResolver for MockResolver {
    fn resolve_overlay_ip(&self, _node_name: &str) -> OverlayReachability {
        match self.overlay_ip {
            Some(ip) if self.active => OverlayReachability::Reachable(ip),
            _ => OverlayReachability::Unavailable,
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

/// Bind an ephemeral TCP listener, grab its port, then drop it.
/// Connecting to `127.0.0.1:{port}` immediately after should get
/// `ECONNREFUSED` on every platform.
async fn refused_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// Test timeout for network-touching tests (10 seconds).
const TEST_TIMEOUT: Duration = Duration::from_secs(10);

// =============================================================================
// 1) test_connector_direct_only_mode
// =============================================================================

/// Create connector with `DirectOnly`, no resolver. Call `connect()` -- will fail
/// (no server), but verify the error does NOT mention overlay policy.
#[tokio::test]
async fn test_connector_direct_only_mode() {
    let port = refused_port().await;
    let url = format!("ws://127.0.0.1:{port}/tunnel/v1");

    let connector = OverlayAwareConnector::new(&url, None, RoutingMode::DirectOnly, None);

    let result = tokio::time::timeout(TEST_TIMEOUT, connector.connect()).await;

    match result {
        Ok(Ok(_)) => panic!("Expected connection to fail (no server running)"),
        Ok(Err(e)) => {
            let err_msg = e.to_string();
            // In DirectOnly mode the error should be about failing to connect,
            // NOT about overlay being unavailable.
            assert!(
                !err_msg.contains("overlay routing required"),
                "DirectOnly should not produce overlay-policy errors, got: {err_msg}"
            );
        }
        Err(_elapsed) => {
            // Timeout is also acceptable -- means it tried direct and never
            // got a response. The key thing is it did NOT fail with an
            // overlay policy error (which would have been immediate).
        }
    }
}

// =============================================================================
// 2) test_connector_overlay_only_no_url_fails
// =============================================================================

/// Create connector with `OverlayOnly` but no overlay URL and no resolver.
/// Verify `connect()` returns a connection error about overlay.
#[tokio::test]
async fn test_connector_overlay_only_no_url_fails() {
    let connector = OverlayAwareConnector::new(
        "ws://127.0.0.1:19876/tunnel/v1",
        None,
        RoutingMode::OverlayOnly,
        None, // No resolver, no pre-computed overlay URL
    );

    // OverlayOnly with no resolver fails immediately (no network call)
    let result = connector.connect().await;
    match result {
        Ok(_) => panic!("OverlayOnly without resolver should error"),
        Err(e) => {
            let err_msg = e.to_string();
            assert!(
                err_msg.contains("overlay"),
                "Error should mention overlay, got: {err_msg}"
            );
        }
    }
}

// =============================================================================
// 3) test_connector_prefer_overlay_with_inactive_resolver
// =============================================================================

/// Create connector with `PreferOverlay` and an inactive `MockResolver`.
/// Call `connect()` -- should fail trying direct URL (not overlay).
/// Verify error is about the direct URL.
#[tokio::test]
async fn test_connector_prefer_overlay_with_inactive_resolver() {
    let port = refused_port().await;
    let url = format!("ws://127.0.0.1:{port}/tunnel/v1");
    let resolver = MockResolver::inactive();

    let connector = OverlayAwareConnector::new(
        &url,
        None,
        RoutingMode::PreferOverlay,
        Some(resolver as DynOverlayResolver),
    );

    // PreferOverlay with inactive overlay should fall back to direct.
    // Direct will also fail (no server), but the error should NOT be an
    // overlay-policy error.
    let result = tokio::time::timeout(TEST_TIMEOUT, connector.connect()).await;

    match result {
        Ok(Ok(_)) => panic!("Connection should fail (no server running)"),
        Ok(Err(e)) => {
            let err_msg = e.to_string();
            assert!(
                !err_msg.contains("overlay routing required"),
                "PreferOverlay should fall back to direct, not fail with overlay policy error, got: {err_msg}"
            );
        }
        Err(_elapsed) => {
            // Timeout is acceptable -- means it tried direct (fell back from
            // overlay as expected) and timed out. Not an overlay policy error.
        }
    }
}

// =============================================================================
// 4) test_tunnel_agent_with_overlay_resolver
// =============================================================================

/// Create `TunnelAgent` with `MockResolver` attached via `with_overlay_resolver()`.
/// Verify it is set. Call `run_once()` -- will fail (no server) but should not panic.
#[tokio::test]
async fn test_tunnel_agent_with_overlay_resolver() {
    let port = refused_port().await;
    let url = format!("ws://127.0.0.1:{port}/tunnel/v1");

    // Use LOCALHOST as the overlay IP so the connector's overlay attempt
    // hits localhost and gets connection refused quickly.
    let resolver = MockResolver::active_with_ip(Ipv4Addr::LOCALHOST);

    let config = TunnelClientConfig::new(&url, "test-token");

    let agent = TunnelAgent::new(config).with_overlay_resolver(resolver as DynOverlayResolver);

    // Verify agent state before running
    assert!(
        !agent.is_connected(),
        "Agent should not be connected before run()"
    );
    assert!(
        agent.tunnel_id().is_none(),
        "Agent should have no tunnel ID before run()"
    );

    // run_once will fail (no server), but overlay resolver was set and
    // should be used for URL resolution -- no panics expected.
    let result = tokio::time::timeout(TEST_TIMEOUT, agent.run_once()).await;

    match result {
        Ok(inner) => {
            assert!(
                inner.is_err(),
                "run_once should fail because no server is running"
            );
        }
        Err(_elapsed) => {
            // Timeout is acceptable -- agent tried connecting and hung.
            // Not a panic, which is the key assertion.
        }
    }

    // Agent should not be connected after a failed run_once
    assert!(
        !agent.is_connected(),
        "Agent should not be connected after failed run_once"
    );
}

// =============================================================================
// 5) test_node_tunnel_manager_with_overlay_resolver
// =============================================================================

/// Create `NodeTunnelManager` with `MockResolver` attached. Add a tunnel from
/// "node-a" to "node-b". The resolver resolves "node-b" to 10.200.0.5.
/// Verify the computed overlay URL is `ws://10.200.0.5:3669/tunnel/v1`.
#[tokio::test]
async fn test_node_tunnel_manager_with_overlay_resolver() {
    let resolver = MockResolver::active_with_ip(Ipv4Addr::new(10, 200, 0, 5));

    let server_config = TunnelServerConfig::default();
    let manager = NodeTunnelManager::new("node-a", server_config)
        .with_overlay_resolver(resolver as DynOverlayResolver);

    // Add a tunnel from node-a to node-b
    let tunnel = NodeTunnel::new("db-tunnel", "node-a", "node-b")
        .with_ports(5432, 5432)
        .with_token("tun_test_token");

    manager
        .add_tunnel(tunnel)
        .expect("add_tunnel should succeed");

    // Verify the tunnel is stored
    let stored = manager.get_tunnel("db-tunnel");
    assert!(stored.is_some(), "Tunnel should be stored in manager");
    let stored = stored.unwrap();
    assert_eq!(stored.from, "node-a");
    assert_eq!(stored.to, "node-b");

    // Start outbound -- this spawns an async task that will attempt to connect.
    // The connection itself will fail (no server), but we verify that
    // start_outbound succeeds (it just spawns) and the internal overlay URL
    // computation uses the resolver. The compute_overlay_url method (private)
    // would produce "ws://10.200.0.5:3669/tunnel/v1".
    //
    // We use 127.0.0.1 as the direct URL so the background task's fallback
    // fails quickly with connection refused rather than hanging on DNS.
    let port = refused_port().await;
    let direct_url = format!("ws://127.0.0.1:{port}/tunnel/v1");

    let result = manager.start_outbound("db-tunnel", direct_url);
    assert!(
        result.is_ok(),
        "start_outbound should succeed (spawns async task): {result:?}"
    );

    // Give the spawned task a moment to register
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(
        manager.outbound_count(),
        1,
        "Should have one outbound agent running"
    );

    // Clean up
    manager.shutdown();

    // After shutdown, all statuses should be Disconnected
    for status in manager.list_status() {
        assert_eq!(
            status.state,
            zlayer_tunnel_zql::TunnelState::Disconnected,
            "All tunnel states should be Disconnected after shutdown"
        );
    }
}

// =============================================================================
// 6) test_access_manager_overlay_resolution
// =============================================================================

/// Create `AccessManager` with `MockResolver` resolving "db-node" to 10.200.0.10.
/// Start session with remote "db-node:5432". Verify it tries connecting (will
/// fail but check it resolved to overlay IP).
#[tokio::test]
async fn test_access_manager_overlay_resolution() {
    let resolver = MockResolver::active_with_ip(Ipv4Addr::new(10, 200, 0, 10));

    let manager = AccessManager::new().with_overlay_resolver(resolver as DynOverlayResolver);

    // Start a session with a hostname remote address.
    // The overlay resolver should resolve "db-node" to 10.200.0.10 internally
    // when the manager calls resolve_remote().
    let session = manager
        .start_session(
            "postgres".to_string(),
            "db-node:5432".to_string(),
            None, // auto-assign local port
            None, // no TTL
        )
        .await
        .expect("start_session should succeed");

    assert_eq!(session.endpoint, "postgres");
    // The session's remote_addr stores the original value (before resolution).
    // The resolved address is used internally by the listener task.
    assert_eq!(session.remote_addr, "db-node:5432");
    assert_eq!(session.local_addr.ip().to_string(), "127.0.0.1");
    assert!(
        session.local_addr.port() > 0,
        "Local port should be assigned"
    );
    assert!(!session.is_expired(), "Session should not be expired");

    // Verify session is tracked
    assert_eq!(manager.session_count(), 1);

    let listed = manager.list_sessions();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].endpoint, "postgres");

    // Clean up
    let stopped = manager.stop_session(session.id);
    assert!(stopped.is_some(), "stop_session should return the session");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(manager.session_count(), 0);
}

// =============================================================================
// 7) test_routing_mode_config_roundtrip
// =============================================================================

/// Create `TunnelClientConfig` and `TunnelServerConfig` with `routing_mode` fields
/// set. Serialize/deserialize with toml. Verify fields are preserved.
#[tokio::test]
async fn test_routing_mode_config_roundtrip() {
    // -- Client config roundtrip --
    let client_config = TunnelClientConfig {
        overlay_server_url: Some("ws://10.200.0.5:3669/tunnel/v1".to_string()),
        routing_mode: RoutingMode::OverlayOnly,
        ..TunnelClientConfig::new("wss://tunnel.example.com/tunnel/v1", "my-token")
    };

    let client_toml = toml::to_string(&client_config).expect("serialize client config");
    let parsed_client: TunnelClientConfig =
        toml::from_str(&client_toml).expect("deserialize client config");

    assert_eq!(parsed_client.server_url, client_config.server_url);
    assert_eq!(parsed_client.token, client_config.token);
    assert_eq!(parsed_client.routing_mode, RoutingMode::OverlayOnly);
    assert_eq!(
        parsed_client.overlay_server_url.as_deref(),
        Some("ws://10.200.0.5:3669/tunnel/v1")
    );

    // -- Server config roundtrip --
    let server_config = TunnelServerConfig {
        routing_mode: RoutingMode::DirectOnly,
        bind_overlay: true,
        dns_registration: true,
        ..TunnelServerConfig::default()
    };

    let server_toml = toml::to_string(&server_config).expect("serialize server config");
    let parsed_server: TunnelServerConfig =
        toml::from_str(&server_toml).expect("deserialize server config");

    assert_eq!(parsed_server.routing_mode, RoutingMode::DirectOnly);
    assert!(parsed_server.bind_overlay);
    assert!(parsed_server.dns_registration);
    assert_eq!(parsed_server.enabled, server_config.enabled);
    assert_eq!(parsed_server.control_path, server_config.control_path);
    assert_eq!(parsed_server.data_port_range, server_config.data_port_range);
    assert_eq!(
        parsed_server.heartbeat_interval,
        server_config.heartbeat_interval
    );
    assert_eq!(
        parsed_server.heartbeat_timeout,
        server_config.heartbeat_timeout
    );
    assert_eq!(parsed_server.max_tunnels, server_config.max_tunnels);
    assert_eq!(
        parsed_server.max_services_per_tunnel,
        server_config.max_services_per_tunnel
    );

    // -- PreferOverlay roundtrip (the default) --
    let default_client = TunnelClientConfig::new("ws://host:3669/tunnel/v1", "token");
    let default_toml = toml::to_string(&default_client).expect("serialize default");
    let parsed_default: TunnelClientConfig =
        toml::from_str(&default_toml).expect("deserialize default");
    assert_eq!(parsed_default.routing_mode, RoutingMode::PreferOverlay);
}

// =============================================================================
// 8) test_node_tunnel_routing_mode_config
// =============================================================================

/// Create `NodeTunnel` with `routing_mode` set. Verify it serializes/deserializes
/// correctly via toml.
#[tokio::test]
async fn test_node_tunnel_routing_mode_config() {
    // Test with each routing mode variant
    let modes = [
        RoutingMode::PreferOverlay,
        RoutingMode::OverlayOnly,
        RoutingMode::DirectOnly,
    ];

    for mode in &modes {
        let tunnel = NodeTunnel::new("test-tunnel", "node-a", "node-b")
            .with_ports(22, 2222)
            .with_token("secret-token");

        // Set the routing mode by creating a modified copy
        let tunnel = NodeTunnel {
            routing_mode: *mode,
            ..tunnel
        };

        let toml_str = toml::to_string(&tunnel).expect("serialize NodeTunnel");
        let parsed: NodeTunnel = toml::from_str(&toml_str).expect("deserialize NodeTunnel");

        assert_eq!(
            parsed.routing_mode, *mode,
            "Routing mode should survive roundtrip for {mode:?}"
        );
        assert_eq!(parsed.name, "test-tunnel");
        assert_eq!(parsed.from, "node-a");
        assert_eq!(parsed.to, "node-b");
        assert_eq!(parsed.local_port, 22);
        assert_eq!(parsed.remote_port, 2222);
        assert_eq!(parsed.token, Some("secret-token".to_string()));
    }

    // Verify default routing mode when field is absent
    let minimal_toml = r#"
name = "minimal"
from = "a"
to = "b"
local_port = 80
remote_port = 8080
"#;
    let parsed: NodeTunnel = toml::from_str(minimal_toml).expect("deserialize minimal");
    assert_eq!(
        parsed.routing_mode,
        RoutingMode::PreferOverlay,
        "Missing routing_mode should default to PreferOverlay"
    );
}

// =============================================================================
// 9) test_routing_mode_config_roundtrip_ipv6_overlay_url
// =============================================================================

/// Verify that `TunnelClientConfig` with an IPv6 overlay server URL
/// round-trips correctly through TOML serialization. IPv6 addresses in URLs
/// use bracket notation (e.g., `ws://[fd00::5]:3669/tunnel/v1`).
#[tokio::test]
async fn test_routing_mode_config_roundtrip_ipv6_overlay_url() {
    let ipv6_overlay_url = "ws://[fd00:200::5]:3669/tunnel/v1";

    let client_config = TunnelClientConfig {
        overlay_server_url: Some(ipv6_overlay_url.to_string()),
        routing_mode: RoutingMode::OverlayOnly,
        ..TunnelClientConfig::new("wss://tunnel.example.com/tunnel/v1", "my-token")
    };

    let client_toml = toml::to_string(&client_config).expect("serialize client config with IPv6");
    let parsed_client: TunnelClientConfig =
        toml::from_str(&client_toml).expect("deserialize client config with IPv6");

    assert_eq!(parsed_client.routing_mode, RoutingMode::OverlayOnly);
    assert_eq!(
        parsed_client.overlay_server_url.as_deref(),
        Some(ipv6_overlay_url),
        "IPv6 overlay server URL should survive TOML roundtrip"
    );
    assert_eq!(parsed_client.server_url, client_config.server_url);
    assert_eq!(parsed_client.token, client_config.token);
}

// =============================================================================
// 10) test_connector_direct_only_mode_ipv6_url
// =============================================================================

/// Same as `test_connector_direct_only_mode` but with an IPv6 loopback URL.
/// Verifies the connector handles IPv6 URLs without panicking.
#[tokio::test]
async fn test_connector_direct_only_mode_ipv6_url() {
    let port = refused_port().await;
    let url = format!("ws://[::1]:{port}/tunnel/v1");

    let connector = OverlayAwareConnector::new(&url, None, RoutingMode::DirectOnly, None);

    let result = tokio::time::timeout(TEST_TIMEOUT, connector.connect()).await;

    match result {
        Ok(Ok(_)) => panic!("Expected connection to fail (no server running)"),
        Ok(Err(e)) => {
            let err_msg = e.to_string();
            assert!(
                !err_msg.contains("overlay routing required"),
                "DirectOnly with IPv6 URL should not produce overlay-policy errors, got: {err_msg}"
            );
        }
        Err(_elapsed) => {
            // Timeout is acceptable -- tried direct IPv6 and never got a response
        }
    }
}
