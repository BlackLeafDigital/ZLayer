//! Built-in ZLayer relay server.
//!
//! A simple UDP relay server that ZLayer nodes can optionally run.
//! Accepts the custom ZLayer relay protocol (see [`super::turn`]) and
//! relays data between authenticated clients.
//!
//! Each allocation gets its own relay UDP port. Clients must authenticate
//! with BLAKE2b-256 MAC on control messages. Data messages are forwarded
//! based on permission tables per allocation.

use crate::error::OverlayError;
use crate::nat::config::RelayServerConfig;
use crate::nat::turn::{
    build_control_msg, build_data_msg, decode_addr_v4, derive_auth_key, encode_addr_v4,
    parse_and_verify_control, parse_data_payload, parse_msg, MsgType, PEER_ADDR_LEN,
};

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rand::Rng;
use tokio::net::UdpSocket;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Default allocation lifetime in seconds.
const DEFAULT_LIFETIME_SECS: u32 = 600;

/// Maximum UDP packet size for relay.
const MAX_PACKET_SIZE: usize = 65536;

// ---- Allocation state -------------------------------------------------------

/// A single relay allocation managed by the server.
#[allow(dead_code)]
struct Allocation {
    /// The client's address (who created this allocation).
    client_addr: SocketAddr,
    /// Server-assigned allocation ID.
    allocation_id: [u8; 16],
    /// The relay socket bound for this allocation.
    relay_socket: Arc<UdpSocket>,
    /// The relay address (external_addr:relay_port).
    relay_addr: SocketAddrV4,
    /// Permitted peer addresses that can send/receive through this allocation.
    permissions: Vec<SocketAddrV4>,
    /// Allocation lifetime in seconds.
    lifetime_secs: u32,
    /// When this allocation was created or last refreshed.
    refreshed_at: std::time::Instant,
    /// Handle to the per-allocation relay task.
    relay_handle: tokio::task::JoinHandle<()>,
}

/// Shared allocation table, keyed by allocation_id.
type AllocationTable = Arc<RwLock<HashMap<[u8; 16], Allocation>>>;

/// Reverse lookup: client address -> allocation ID.
type ClientLookup = Arc<RwLock<HashMap<SocketAddr, [u8; 16]>>>;

// ---- Relay server -----------------------------------------------------------

/// A built-in relay server for the ZLayer custom relay protocol.
///
/// Nodes can optionally run this to provide relay services for peers
/// behind restrictive NATs.
pub struct RelayServer {
    config: RelayServerConfig,
    auth_key: [u8; 32],
    shutdown: Arc<AtomicBool>,
}

impl RelayServer {
    /// Create a new relay server.
    ///
    /// # Arguments
    ///
    /// * `config` - Relay server configuration (port, external addr, max sessions).
    /// * `auth_credential` - Credential string used to derive the BLAKE2b-256 auth key.
    #[must_use]
    pub fn new(config: &RelayServerConfig, auth_credential: &str) -> Self {
        Self {
            config: config.clone(),
            auth_key: derive_auth_key(auth_credential),
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Start the relay server.
    ///
    /// Listens on `0.0.0.0:{listen_port}` for control and data messages.
    /// Spawns per-allocation relay tasks for data forwarding.
    ///
    /// This method spawns the server loop as a background tokio task and
    /// returns immediately.
    ///
    /// # Errors
    ///
    /// Returns [`OverlayError::TurnRelay`] if the listen socket cannot be bound.
    pub async fn start(&self) -> Result<(), OverlayError> {
        let listen_addr = SocketAddr::new(
            std::net::IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            self.config.listen_port,
        );

        let socket = Arc::new(UdpSocket::bind(listen_addr).await.map_err(|e| {
            OverlayError::TurnRelay(format!("Failed to bind relay server on {listen_addr}: {e}"))
        })?);

        info!(
            listen = %listen_addr,
            external = %self.config.external_addr,
            max_sessions = self.config.max_sessions,
            "Relay server started"
        );

        let allocations: AllocationTable = Arc::new(RwLock::new(HashMap::new()));
        let client_lookup: ClientLookup = Arc::new(RwLock::new(HashMap::new()));
        let auth_key = self.auth_key;
        let external_addr: SocketAddr = self
            .config
            .external_addr
            .parse()
            .map_err(|e| OverlayError::TurnRelay(format!("Invalid external addr: {e}")))?;
        let max_sessions = self.config.max_sessions;
        let shutdown = self.shutdown.clone();
        let socket_clone = socket.clone();

        tokio::spawn(async move {
            let mut buf = [0u8; MAX_PACKET_SIZE];

            loop {
                if shutdown.load(Ordering::Relaxed) {
                    info!("Relay server shutting down");
                    break;
                }

                let recv_result = tokio::time::timeout(
                    std::time::Duration::from_secs(1),
                    socket_clone.recv_from(&mut buf),
                )
                .await;

                let (n, from) = match recv_result {
                    Ok(Ok((n, from))) => (n, from),
                    Ok(Err(e)) => {
                        warn!(error = %e, "Relay server recv error");
                        continue;
                    }
                    Err(_) => continue, // Timeout, check shutdown flag
                };

                let packet = &buf[..n];

                // Determine message type
                let Some(msg_type_byte) = packet.first() else {
                    continue;
                };
                let Some(msg_type) = MsgType::from_byte(*msg_type_byte) else {
                    continue;
                };

                match msg_type {
                    MsgType::AllocateReq => {
                        handle_allocate_req(
                            packet,
                            from,
                            &auth_key,
                            external_addr,
                            max_sessions,
                            &allocations,
                            &client_lookup,
                            &socket_clone,
                        )
                        .await;
                    }
                    MsgType::PermissionReq => {
                        handle_permission_req(packet, from, &auth_key, &allocations, &socket_clone)
                            .await;
                    }
                    MsgType::RefreshReq => {
                        handle_refresh_req(packet, from, &auth_key, &allocations, &socket_clone)
                            .await;
                    }
                    MsgType::Deallocate => {
                        handle_deallocate(packet, from, &auth_key, &allocations, &client_lookup)
                            .await;
                    }
                    MsgType::Data => {
                        handle_data(packet, from, &allocations, &client_lookup).await;
                    }
                    _ => {
                        debug!(msg_type = ?msg_type, from = %from, "Ignoring unexpected message type");
                    }
                }
            }

            // Cleanup: abort all relay tasks
            let allocs = allocations.write().await;
            for (_, alloc) in allocs.iter() {
                alloc.relay_handle.abort();
            }
        });

        Ok(())
    }

    /// Signal the relay server to shut down.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
        info!("Relay server shutdown signaled");
    }
}

// ---- Handler functions ------------------------------------------------------

/// Handle an AllocateReq message.
#[allow(clippy::too_many_arguments)]
async fn handle_allocate_req(
    packet: &[u8],
    from: SocketAddr,
    auth_key: &[u8; 32],
    external_addr: SocketAddr,
    max_sessions: usize,
    allocations: &AllocationTable,
    client_lookup: &ClientLookup,
    main_socket: &Arc<UdpSocket>,
) {
    // Parse and verify
    let Some((_msg_type, payload)) = parse_and_verify_control(packet, auth_key) else {
        debug!(from = %from, "AllocateReq auth failed");
        return;
    };

    // Check session limit
    {
        let allocs = allocations.read().await;
        if allocs.len() >= max_sessions {
            let err_msg =
                build_control_msg(MsgType::AllocateErr, b"max sessions reached", auth_key);
            let _ = main_socket.send_to(&err_msg, from).await;
            return;
        }
    }

    // Check if client already has an allocation
    {
        let lookup = client_lookup.read().await;
        if lookup.contains_key(&from) {
            let err_msg =
                build_control_msg(MsgType::AllocateErr, b"allocation already exists", auth_key);
            let _ = main_socket.send_to(&err_msg, from).await;
            return;
        }
    }

    // Parse username from payload (we don't use it for auth, just logging)
    let username = if !payload.is_empty() {
        let ulen = payload[0] as usize;
        if payload.len() > ulen {
            String::from_utf8_lossy(&payload[1..=ulen]).to_string()
        } else {
            "unknown".to_string()
        }
    } else {
        "unknown".to_string()
    };

    // Bind a new UDP socket for this allocation's relay port
    let relay_socket = match UdpSocket::bind("0.0.0.0:0").await {
        Ok(s) => Arc::new(s),
        Err(e) => {
            warn!(error = %e, "Failed to bind relay socket for allocation");
            let err_msg = build_control_msg(MsgType::AllocateErr, b"relay bind failed", auth_key);
            let _ = main_socket.send_to(&err_msg, from).await;
            return;
        }
    };

    let relay_port = match relay_socket.local_addr() {
        Ok(addr) => addr.port(),
        Err(e) => {
            warn!(error = %e, "Failed to get relay socket addr");
            return;
        }
    };

    // Build relay address using external_addr's IP + the relay port
    let external_ip = match external_addr {
        SocketAddr::V4(v4) => *v4.ip(),
        SocketAddr::V6(_) => {
            warn!("IPv6 external addr not supported for relay");
            return;
        }
    };
    let relay_addr = SocketAddrV4::new(external_ip, relay_port);

    // Generate allocation ID
    let mut allocation_id = [0u8; 16];
    rand::rng().fill(&mut allocation_id[..]);

    // Spawn per-allocation relay task: listens on relay_socket for peer data,
    // wraps it and sends to the client through the main socket
    let relay_socket_clone = relay_socket.clone();
    let main_socket_clone = main_socket.clone();
    let alloc_table_clone = allocations.clone();
    let alloc_id_copy = allocation_id;
    let client_addr = from;

    let relay_handle = tokio::spawn(async move {
        let mut buf = [0u8; MAX_PACKET_SIZE];

        loop {
            match relay_socket_clone.recv_from(&mut buf).await {
                Ok((n, peer_from)) => {
                    // Check that this peer has a permission
                    let permitted = {
                        let allocs = alloc_table_clone.read().await;
                        if let Some(alloc) = allocs.get(&alloc_id_copy) {
                            let peer_v4 = match peer_from {
                                SocketAddr::V4(v4) => v4,
                                SocketAddr::V6(_) => continue,
                            };
                            alloc.permissions.iter().any(|p| p.ip() == peer_v4.ip())
                        } else {
                            break; // Allocation removed
                        }
                    };

                    if !permitted {
                        continue;
                    }

                    // Wrap peer data as DATA message and send to client
                    let peer_v4 = match peer_from {
                        SocketAddr::V4(v4) => v4,
                        SocketAddr::V6(_) => continue,
                    };
                    let data_msg = build_data_msg(peer_v4, &buf[..n]);
                    if let Err(e) = main_socket_clone.send_to(&data_msg, client_addr).await {
                        warn!(error = %e, "Failed to forward peer data to client");
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Relay socket recv error");
                    break;
                }
            }
        }
    });

    // Store the allocation
    let allocation = Allocation {
        client_addr: from,
        allocation_id,
        relay_socket,
        relay_addr,
        permissions: Vec::new(),
        lifetime_secs: DEFAULT_LIFETIME_SECS,
        refreshed_at: std::time::Instant::now(),
        relay_handle,
    };

    {
        let mut allocs = allocations.write().await;
        allocs.insert(allocation_id, allocation);
    }
    {
        let mut lookup = client_lookup.write().await;
        lookup.insert(from, allocation_id);
    }

    // Build AllocateResp: [relay_addr: 6] [allocation_id: 16] [lifetime: 4]
    let mut resp_payload = Vec::with_capacity(PEER_ADDR_LEN + 16 + 4);
    resp_payload.extend_from_slice(&encode_addr_v4(relay_addr));
    resp_payload.extend_from_slice(&allocation_id);
    resp_payload.extend_from_slice(&DEFAULT_LIFETIME_SECS.to_be_bytes());

    let resp = build_control_msg(MsgType::AllocateResp, &resp_payload, auth_key);
    let _ = main_socket.send_to(&resp, from).await;

    info!(
        client = %from,
        relay = %relay_addr,
        username = %username,
        "Relay allocation created"
    );
}

/// Handle a PermissionReq message.
async fn handle_permission_req(
    packet: &[u8],
    from: SocketAddr,
    auth_key: &[u8; 32],
    allocations: &AllocationTable,
    main_socket: &Arc<UdpSocket>,
) {
    let Some((_msg_type, payload)) = parse_and_verify_control(packet, auth_key) else {
        debug!(from = %from, "PermissionReq auth failed");
        return;
    };

    if payload.len() < 16 + PEER_ADDR_LEN {
        return;
    }

    let mut alloc_id = [0u8; 16];
    alloc_id.copy_from_slice(&payload[..16]);

    let Some(peer_addr) = decode_addr_v4(&payload[16..16 + PEER_ADDR_LEN]) else {
        return;
    };

    // Add permission
    {
        let mut allocs = allocations.write().await;
        if let Some(alloc) = allocs.get_mut(&alloc_id) {
            if alloc.client_addr != from {
                debug!(from = %from, "PermissionReq from non-owner");
                return;
            }
            if !alloc.permissions.contains(&peer_addr) {
                alloc.permissions.push(peer_addr);
            }
        } else {
            debug!(from = %from, "PermissionReq for unknown allocation");
            return;
        }
    }

    // Send PermissionResp (empty payload)
    let resp = build_control_msg(MsgType::PermissionResp, &[], auth_key);
    let _ = main_socket.send_to(&resp, from).await;

    debug!(from = %from, peer = %peer_addr, "Permission added");
}

/// Handle a RefreshReq message.
async fn handle_refresh_req(
    packet: &[u8],
    from: SocketAddr,
    auth_key: &[u8; 32],
    allocations: &AllocationTable,
    main_socket: &Arc<UdpSocket>,
) {
    let Some((_msg_type, payload)) = parse_and_verify_control(packet, auth_key) else {
        debug!(from = %from, "RefreshReq auth failed");
        return;
    };

    if payload.len() < 16 + 4 {
        return;
    }

    let mut alloc_id = [0u8; 16];
    alloc_id.copy_from_slice(&payload[..16]);

    let lifetime = u32::from_be_bytes([payload[16], payload[17], payload[18], payload[19]]);

    // Refresh allocation
    {
        let mut allocs = allocations.write().await;
        if let Some(alloc) = allocs.get_mut(&alloc_id) {
            if alloc.client_addr != from {
                debug!(from = %from, "RefreshReq from non-owner");
                return;
            }
            alloc.lifetime_secs = lifetime;
            alloc.refreshed_at = std::time::Instant::now();
        } else {
            debug!(from = %from, "RefreshReq for unknown allocation");
            return;
        }
    }

    // Send RefreshResp with confirmed lifetime
    let resp = build_control_msg(MsgType::RefreshResp, &lifetime.to_be_bytes(), auth_key);
    let _ = main_socket.send_to(&resp, from).await;

    debug!(from = %from, lifetime = lifetime, "Allocation refreshed");
}

/// Handle a Deallocate message.
async fn handle_deallocate(
    packet: &[u8],
    from: SocketAddr,
    auth_key: &[u8; 32],
    allocations: &AllocationTable,
    client_lookup: &ClientLookup,
) {
    let Some((_msg_type, payload)) = parse_and_verify_control(packet, auth_key) else {
        debug!(from = %from, "Deallocate auth failed");
        return;
    };

    if payload.len() < 16 {
        return;
    }

    let mut alloc_id = [0u8; 16];
    alloc_id.copy_from_slice(&payload[..16]);

    // Remove allocation
    let removed = {
        let mut allocs = allocations.write().await;
        if let Some(alloc) = allocs.get(&alloc_id) {
            if alloc.client_addr != from {
                debug!(from = %from, "Deallocate from non-owner");
                return;
            }
        }
        allocs.remove(&alloc_id)
    };

    if let Some(alloc) = removed {
        alloc.relay_handle.abort();
        let mut lookup = client_lookup.write().await;
        lookup.remove(&from);
        info!(client = %from, "Allocation deallocated");
    }
}

/// Handle a Data message from a client.
async fn handle_data(
    packet: &[u8],
    from: SocketAddr,
    allocations: &AllocationTable,
    client_lookup: &ClientLookup,
) {
    // Look up the client's allocation
    let alloc_id = {
        let lookup = client_lookup.read().await;
        match lookup.get(&from) {
            Some(id) => *id,
            None => return, // Unknown client
        }
    };

    // Parse DATA message
    let Some((MsgType::Data, payload)) = parse_msg(packet) else {
        return;
    };
    let Some((peer_addr, raw_data)) = parse_data_payload(payload) else {
        return;
    };

    // Forward data through the allocation's relay socket
    let allocs = allocations.read().await;
    if let Some(alloc) = allocs.get(&alloc_id) {
        // Check permission
        if !alloc.permissions.iter().any(|p| p.ip() == peer_addr.ip()) {
            return;
        }

        let dest = SocketAddr::V4(peer_addr);
        if let Err(e) = alloc.relay_socket.send_to(raw_data, dest).await {
            warn!(error = %e, dest = %dest, "Failed to relay data to peer");
        }
    }
}

// ---- Tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nat::config::TurnServerConfig;

    #[test]
    fn test_relay_server_new() {
        let config = RelayServerConfig {
            listen_port: 3478,
            external_addr: "1.2.3.4:3478".to_string(),
            max_sessions: 50,
        };
        let server = RelayServer::new(&config, "test_credential");
        assert_eq!(server.config.listen_port, 3478);
        assert_eq!(server.config.max_sessions, 50);
    }

    #[test]
    fn test_relay_server_auth_key_derivation() {
        let config = RelayServerConfig {
            listen_port: 3478,
            external_addr: "1.2.3.4:3478".to_string(),
            max_sessions: 100,
        };
        let server = RelayServer::new(&config, "shared_secret");
        let expected_key = derive_auth_key("shared_secret");
        assert_eq!(server.auth_key, expected_key);
    }

    #[test]
    fn test_relay_server_shutdown_flag() {
        let config = RelayServerConfig {
            listen_port: 3478,
            external_addr: "1.2.3.4:3478".to_string(),
            max_sessions: 100,
        };
        let server = RelayServer::new(&config, "test");
        assert!(!server.shutdown.load(Ordering::Relaxed));
        server.shutdown();
        assert!(server.shutdown.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn test_relay_server_allocate_roundtrip() {
        // Start relay server
        let _config = RelayServerConfig {
            listen_port: 0, // OS picks port
            external_addr: "127.0.0.1:0".to_string(),
            max_sessions: 10,
        };
        let credential = "test_secret";

        // We need to bind first to know the port, then start the server
        let listen_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let listen_port = listen_socket.local_addr().unwrap().port();
        drop(listen_socket);

        let real_config = RelayServerConfig {
            listen_port,
            external_addr: format!("127.0.0.1:{listen_port}"),
            max_sessions: 10,
        };

        let server = RelayServer::new(&real_config, credential);
        server.start().await.unwrap();

        // Give server time to bind
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Create client and allocate
        let client_config = TurnServerConfig {
            address: format!("127.0.0.1:{listen_port}"),
            username: "testuser".to_string(),
            credential: credential.to_string(),
            region: None,
        };

        let mut client = crate::nat::turn::RelayClient::new(&client_config).unwrap();
        let result = client.allocate().await;

        // Should succeed
        assert!(result.is_ok(), "Allocation failed: {result:?}");
        assert!(client.is_active());

        // Clean up
        let _ = client.deallocate().await;
        server.shutdown();
    }
}
