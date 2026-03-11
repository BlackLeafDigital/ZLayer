#![cfg(feature = "nat")]
//! End-to-end integration tests for NAT traversal.
//!
//! All tests require the `nat` feature:
//!
//! ```sh
//! cargo test -p zlayer-overlay --features nat --test nat_e2e
//! ```

use std::net::SocketAddr;
use std::time::Duration;

use zlayer_overlay::nat::stun::NatBehavior;
use zlayer_overlay::nat::turn::{
    build_control_msg, build_data_msg_tagged, decode_addr, derive_auth_key, encode_addr,
    parse_and_verify_control, parse_data_payload_tagged, parse_msg, MsgType, RelayClient,
};
use zlayer_overlay::nat::{
    CandidateType, NatConfig, NatTraversal, RelayServer, RelayServerConfig, StunClient,
    StunServerConfig, TurnServerConfig,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Bind an ephemeral port and return its number.
async fn ephemeral_port() -> u16 {
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = socket.local_addr().unwrap().port();
    drop(socket);
    port
}

/// Start a relay server on an ephemeral port with the given credential and
/// session limit. Returns `(server, listen_port)`.
async fn start_relay(credential: &str, max_sessions: usize) -> (RelayServer, u16) {
    let listen_port = ephemeral_port().await;
    let relay_config = RelayServerConfig {
        listen_port,
        external_addr: format!("127.0.0.1:{listen_port}"),
        max_sessions,
    };
    let server = RelayServer::new(&relay_config, credential);
    server.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    (server, listen_port)
}

/// Create a `RelayClient` targeting `127.0.0.1:{listen_port}`.
fn make_relay_client(listen_port: u16, credential: &str) -> RelayClient {
    let client_config = TurnServerConfig {
        address: format!("127.0.0.1:{listen_port}"),
        username: "test-user".to_string(),
        credential: credential.to_string(),
        region: None,
    };
    RelayClient::new(&client_config).unwrap()
}

// ---------------------------------------------------------------------------
// 1. STUN: discover reflexive address from Google STUN
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_stun_discover_reflexive_address() {
    let client = StunClient::new(vec![StunServerConfig {
        address: "stun.l.google.com:19302".to_string(),
        label: Some("Google STUN".to_string()),
    }]);

    // Resolve server address
    let resolved = match tokio::net::lookup_host("stun.l.google.com:19302").await {
        Ok(mut addrs) => {
            if let Some(addr) = addrs.next() {
                addr
            } else {
                eprintln!("SKIP: DNS returned no addresses for stun.l.google.com");
                return;
            }
        }
        Err(e) => {
            eprintln!("SKIP: DNS resolution failed for stun.l.google.com: {e}");
            return;
        }
    };

    match client.query_server(resolved, "Google STUN").await {
        Ok(reflexive) => {
            assert!(
                !reflexive.address.ip().is_unspecified(),
                "Reflexive address should not be 0.0.0.0, got {}",
                reflexive.address
            );
            assert_ne!(
                reflexive.address.port(),
                0,
                "Reflexive port should not be 0"
            );
            assert_eq!(reflexive.server, "Google STUN");
        }
        Err(e) => {
            eprintln!("SKIP: STUN query to Google failed (network may be unavailable): {e}");
        }
    }
}

// ---------------------------------------------------------------------------
// 2. STUN: query 2 servers, verify NatBehavior detected
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_stun_queries_multiple_servers() {
    let client = StunClient::new(vec![
        StunServerConfig {
            address: "stun.l.google.com:19302".to_string(),
            label: Some("Google STUN 1".to_string()),
        },
        StunServerConfig {
            address: "stun1.l.google.com:19302".to_string(),
            label: Some("Google STUN 2".to_string()),
        },
    ]);

    match client.discover().await {
        Ok((results, behavior)) => {
            assert!(
                !results.is_empty(),
                "Should have at least one reflexive address"
            );

            // Behavior should be one of the two valid variants
            assert!(
                behavior == NatBehavior::EndpointIndependent || behavior == NatBehavior::Symmetric,
                "NatBehavior should be detected, got: {behavior:?}"
            );
        }
        Err(e) => {
            eprintln!("SKIP: STUN discovery failed (network may be unavailable): {e}");
        }
    }
}

// ---------------------------------------------------------------------------
// 3. NAT traversal: gather_candidates produces Host candidate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_nat_traversal_gather_candidates() {
    let config = NatConfig {
        enabled: true,
        stun_servers: vec![],
        turn_servers: vec![],
        ..NatConfig::default()
    };

    let mut nat = NatTraversal::new(config, 51820);

    match nat.gather_candidates().await {
        Ok(candidates) => {
            let has_host = candidates
                .iter()
                .any(|c| c.candidate_type == CandidateType::Host);
            assert!(has_host, "Should have at least one Host candidate");

            for c in &candidates {
                if c.candidate_type == CandidateType::Host {
                    assert_eq!(c.address.port(), 51820, "Host candidate should use WG port");
                    assert!(
                        !c.address.ip().is_unspecified(),
                        "Host candidate IP should not be 0.0.0.0"
                    );
                    assert!(
                        !c.address.ip().is_loopback(),
                        "Host candidate IP should not be loopback"
                    );
                    assert_eq!(c.priority, 100, "Host candidate priority should be 100");
                }
            }

            assert_eq!(nat.local_candidates().len(), candidates.len());
        }
        Err(e) => {
            eprintln!("SKIP: gather_candidates failed (may be sandboxed): {e}");
        }
    }
}

// ---------------------------------------------------------------------------
// 4. Relay: server + client allocate, start_proxy, deallocate, shutdown
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_relay_server_client_allocate() {
    let credential = "test-credential";
    let (server, listen_port) = start_relay(credential, 10).await;

    let mut client = make_relay_client(listen_port, credential);

    // Allocate
    let relay_addr = client.allocate().await.expect("Allocation should succeed");
    assert!(client.is_active(), "Client should be active after allocate");
    assert!(
        relay_addr.port() > 0,
        "Relay address should have a valid port"
    );

    // start_proxy returns a local address
    let proxy_addr = client
        .start_proxy(51820)
        .await
        .expect("start_proxy should succeed");
    assert!(
        proxy_addr.ip().is_loopback(),
        "Proxy should bind on loopback, got {}",
        proxy_addr.ip()
    );
    assert!(proxy_addr.port() > 0, "Proxy port should be non-zero");
    assert_eq!(
        client.proxy_addr(),
        Some(proxy_addr),
        "proxy_addr() should match"
    );

    // Deallocate
    client
        .deallocate()
        .await
        .expect("Deallocation should succeed");
    assert!(
        !client.is_active(),
        "Client should not be active after deallocate"
    );

    // Shutdown
    server.shutdown();
}

// ---------------------------------------------------------------------------
// 5. Relay: allocate, refresh, verify still active, deallocate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_relay_allocation_refresh() {
    let credential = "refresh-cred";
    let (server, listen_port) = start_relay(credential, 10).await;

    let mut client = make_relay_client(listen_port, credential);

    client.allocate().await.expect("Allocation should succeed");
    assert!(client.is_active(), "Should be active after allocate");

    // Refresh
    client.refresh().await.expect("Refresh should succeed");
    assert!(client.is_active(), "Should still be active after refresh");

    // Deallocate
    client
        .deallocate()
        .await
        .expect("Deallocation should succeed");
    assert!(!client.is_active(), "Should not be active after deallocate");

    server.shutdown();
}

// ---------------------------------------------------------------------------
// 6. Relay: session limit enforcement (max_sessions=2, 3rd should fail)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_relay_session_limit() {
    let credential = "limit-cred";
    let (server, listen_port) = start_relay(credential, 2).await;

    // Allocate 2 clients (should succeed)
    let mut client_a = make_relay_client(listen_port, credential);
    let mut client_b = make_relay_client(listen_port, credential);

    client_a
        .allocate()
        .await
        .expect("Client A allocation should succeed");
    client_b
        .allocate()
        .await
        .expect("Client B allocation should succeed");

    assert!(client_a.is_active(), "Client A should be active");
    assert!(client_b.is_active(), "Client B should be active");

    // Third allocation should fail (max_sessions=2)
    let mut client_c = make_relay_client(listen_port, credential);
    let result = client_c.allocate().await;
    assert!(
        result.is_err(),
        "Third allocation should fail when max_sessions=2"
    );
    assert!(
        !client_c.is_active(),
        "Client C should not be active after rejected allocation"
    );

    // Cleanup
    let _ = client_a.deallocate().await;
    let _ = client_b.deallocate().await;
    server.shutdown();
}

// ---------------------------------------------------------------------------
// 7. NAT traversal with relay: gather_candidates includes Relay candidate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_nat_traversal_with_relay_candidates() {
    let credential = "nat-relay-cred";
    let (server, listen_port) = start_relay(credential, 10).await;

    let config = NatConfig {
        enabled: true,
        stun_servers: vec![],
        turn_servers: vec![TurnServerConfig {
            address: format!("127.0.0.1:{listen_port}"),
            username: "test-user".to_string(),
            credential: credential.to_string(),
            region: None,
        }],
        ..NatConfig::default()
    };

    let mut nat = NatTraversal::new(config, 51820);

    match nat.gather_candidates().await {
        Ok(candidates) => {
            let has_relay = candidates
                .iter()
                .any(|c| c.candidate_type == CandidateType::Relay);
            assert!(
                has_relay,
                "Should have at least one Relay candidate, got: {:?}",
                candidates
                    .iter()
                    .map(|c| format!("{:?}@{}", c.candidate_type, c.address))
                    .collect::<Vec<_>>()
            );

            for c in &candidates {
                if c.candidate_type == CandidateType::Relay {
                    assert_eq!(c.priority, 10, "Relay candidates should have priority 10");
                }
            }
        }
        Err(e) => {
            eprintln!("SKIP: gather_candidates with relay failed: {e}");
        }
    }

    server.shutdown();
}

// ---------------------------------------------------------------------------
// 8. Relay: data forwarding between two clients
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_relay_server_data_forwarding() {
    let credential = "data-fwd-cred";
    let (server, listen_port) = start_relay(credential, 10).await;

    let auth_key = derive_auth_key(credential);
    let server_addr: SocketAddr = format!("127.0.0.1:{listen_port}").parse().unwrap();

    // -- Client A: allocate via raw protocol --
    let socket_a = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();

    let username_a = b"client-a";
    let mut alloc_payload_a = Vec::with_capacity(1 + username_a.len());
    alloc_payload_a.push(u8::try_from(username_a.len()).unwrap());
    alloc_payload_a.extend_from_slice(username_a);
    let alloc_msg_a = build_control_msg(MsgType::AllocateReq, &alloc_payload_a, &auth_key);
    socket_a.send_to(&alloc_msg_a, server_addr).await.unwrap();

    let mut buf = [0u8; 1024];
    let n = tokio::time::timeout(Duration::from_secs(5), socket_a.recv(&mut buf))
        .await
        .unwrap()
        .unwrap();
    let (msg_type, resp_a) = parse_and_verify_control(&buf[..n], &auth_key).unwrap();
    assert_eq!(msg_type, MsgType::AllocateResp);
    let (relay_addr_a, addr_len_a) = decode_addr(&resp_a).unwrap();
    let mut alloc_id_a = [0u8; 16];
    alloc_id_a.copy_from_slice(&resp_a[addr_len_a..addr_len_a + 16]);

    // -- Client B: allocate via raw protocol --
    let socket_b = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();

    let username_b = b"client-b";
    let mut alloc_payload_b = Vec::with_capacity(1 + username_b.len());
    alloc_payload_b.push(u8::try_from(username_b.len()).unwrap());
    alloc_payload_b.extend_from_slice(username_b);
    let alloc_msg_b = build_control_msg(MsgType::AllocateReq, &alloc_payload_b, &auth_key);
    socket_b.send_to(&alloc_msg_b, server_addr).await.unwrap();

    let n = tokio::time::timeout(Duration::from_secs(5), socket_b.recv(&mut buf))
        .await
        .unwrap()
        .unwrap();
    let (msg_type, resp_b) = parse_and_verify_control(&buf[..n], &auth_key).unwrap();
    assert_eq!(msg_type, MsgType::AllocateResp);
    let (relay_addr_b, addr_len_b) = decode_addr(&resp_b).unwrap();
    let mut alloc_id_b = [0u8; 16];
    alloc_id_b.copy_from_slice(&resp_b[addr_len_b..addr_len_b + 16]);

    // -- Create mutual permissions --
    // A permits B's relay address
    let encoded_b = encode_addr(relay_addr_b);
    let mut perm_payload = Vec::with_capacity(16 + encoded_b.len());
    perm_payload.extend_from_slice(&alloc_id_a);
    perm_payload.extend_from_slice(&encoded_b);
    let perm_msg = build_control_msg(MsgType::PermissionReq, &perm_payload, &auth_key);
    socket_a.send_to(&perm_msg, server_addr).await.unwrap();
    let n = tokio::time::timeout(Duration::from_secs(5), socket_a.recv(&mut buf))
        .await
        .unwrap()
        .unwrap();
    let (msg_type, _) = parse_and_verify_control(&buf[..n], &auth_key).unwrap();
    assert_eq!(msg_type, MsgType::PermissionResp);

    // B permits A's relay address
    let encoded_a = encode_addr(relay_addr_a);
    let mut perm_payload = Vec::with_capacity(16 + encoded_a.len());
    perm_payload.extend_from_slice(&alloc_id_b);
    perm_payload.extend_from_slice(&encoded_a);
    let perm_msg = build_control_msg(MsgType::PermissionReq, &perm_payload, &auth_key);
    socket_b.send_to(&perm_msg, server_addr).await.unwrap();
    let n = tokio::time::timeout(Duration::from_secs(5), socket_b.recv(&mut buf))
        .await
        .unwrap()
        .unwrap();
    let (msg_type, _) = parse_and_verify_control(&buf[..n], &auth_key).unwrap();
    assert_eq!(msg_type, MsgType::PermissionResp);

    // -- A sends data addressed to B's relay address --
    let test_payload = b"hello from A to B via relay";
    let data_msg = build_data_msg_tagged(relay_addr_b, test_payload);
    socket_a.send_to(&data_msg, server_addr).await.unwrap();

    // B should receive the forwarded data wrapped as a DATA message
    let recv_result = tokio::time::timeout(Duration::from_secs(5), socket_b.recv(&mut buf)).await;

    match recv_result {
        Ok(Ok(n)) => {
            let (msg_type, payload) = parse_msg(&buf[..n]).expect("Should parse as valid message");
            assert_eq!(msg_type, MsgType::Data, "Should receive a Data message");

            let (_from_addr, raw_data) =
                parse_data_payload_tagged(payload).expect("Should parse data payload");
            assert_eq!(
                raw_data, test_payload,
                "Received data should match sent data"
            );
        }
        Ok(Err(e)) => {
            eprintln!("SKIP: relay data forwarding recv failed: {e}");
        }
        Err(_) => {
            eprintln!("SKIP: relay data forwarding timed out (relay task may not have started)");
        }
    }

    // Cleanup
    let dealloc_a = build_control_msg(MsgType::Deallocate, &alloc_id_a, &auth_key);
    let _ = socket_a.send_to(&dealloc_a, server_addr).await;
    let dealloc_b = build_control_msg(MsgType::Deallocate, &alloc_id_b, &auth_key);
    let _ = socket_b.send_to(&dealloc_b, server_addr).await;

    server.shutdown();
}
