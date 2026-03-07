//! Custom `ZLayer` relay protocol client.
//!
//! Implements a lightweight relay client using BLAKE2b-256 authentication
//! instead of full RFC 5766 TURN. `ZLayer` nodes only talk to other `ZLayer`
//! nodes, so standard TURN interoperability is unnecessary.
//!
//! # Protocol
//!
//! All messages are big-endian:
//! - Header: 1 byte type + 2 bytes payload length + payload
//! - Control types: `AllocateReq`(0x01), `AllocateResp`(0x02), `AllocateErr`(0x03),
//!   `PermissionReq`(0x04), `PermissionResp`(0x05), `RefreshReq`(0x06),
//!   `RefreshResp`(0x07), `Deallocate`(0x08)
//! - Data type: `Data`(0x80) -- 1 byte type + 2 bytes length + 6 bytes peer addr
//!   (4 IPv4 + 2 port) + raw data
//!
//! # Authentication
//!
//! BLAKE2b-256 MAC. Key = BLAKE2b-256(credential). Auth tag =
//! `Blake2bMac256`(key, `message_bytes_before_tag`). The 32-byte tag is
//! appended to control messages.

use crate::error::OverlayError;
use crate::nat::candidate::{Candidate, CandidateType};
use crate::nat::config::TurnServerConfig;

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::Instant;

use blake2::digest::consts::U32;
use blake2::digest::{Digest, KeyInit, Mac};
use blake2::{Blake2b, Blake2bMac};
use tokio::net::UdpSocket;
use tracing::{debug, warn};

// ---- Protocol message types ------------------------------------------------

/// Control message types for the `ZLayer` relay protocol.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgType {
    AllocateReq = 0x01,
    AllocateResp = 0x02,
    AllocateErr = 0x03,
    PermissionReq = 0x04,
    PermissionResp = 0x05,
    RefreshReq = 0x06,
    RefreshResp = 0x07,
    Deallocate = 0x08,
    Data = 0x80,
}

impl MsgType {
    /// Parse a byte into a message type.
    #[must_use]
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0x01 => Some(Self::AllocateReq),
            0x02 => Some(Self::AllocateResp),
            0x03 => Some(Self::AllocateErr),
            0x04 => Some(Self::PermissionReq),
            0x05 => Some(Self::PermissionResp),
            0x06 => Some(Self::RefreshReq),
            0x07 => Some(Self::RefreshResp),
            0x08 => Some(Self::Deallocate),
            0x80 => Some(Self::Data),
            _ => None,
        }
    }
}

/// BLAKE2b-256 MAC type alias.
type Blake2bMac256 = Blake2bMac<U32>;

/// BLAKE2b-256 hash type alias.
type Blake2b256 = Blake2b<U32>;

/// Size of the BLAKE2b-256 auth tag appended to control messages.
pub(crate) const AUTH_TAG_LEN: usize = 32;

/// Minimum relay protocol header: 1 byte type + 2 bytes length.
pub(crate) const HEADER_LEN: usize = 3;

/// Size of an encoded IPv4 peer address (4 IP + 2 port).
pub(crate) const PEER_ADDR_LEN: usize = 6;

// ---- Auth helpers -----------------------------------------------------------

/// Derive an auth key from a credential string via BLAKE2b-256 hashing.
#[must_use]
pub fn derive_auth_key(credential: &str) -> [u8; 32] {
    let hash = Blake2b256::digest(credential.as_bytes());
    let mut key = [0u8; 32];
    key.copy_from_slice(&hash);
    key
}

/// Compute a BLAKE2b-256 MAC tag over `data` using `key`.
///
/// # Panics
///
/// Panics if BLAKE2b-256 MAC initialization fails (should never happen with a 32-byte key).
#[must_use]
pub fn compute_auth_tag(key: &[u8; 32], data: &[u8]) -> [u8; 32] {
    let mut mac = <Blake2bMac256 as KeyInit>::new_from_slice(key)
        .expect("BLAKE2bMac256 accepts 32-byte keys");
    mac.update(data);
    let result = mac.finalize();
    let mut tag = [0u8; 32];
    tag.copy_from_slice(&result.into_bytes());
    tag
}

/// Verify a BLAKE2b-256 MAC tag. Returns `true` if valid.
///
/// # Panics
///
/// Panics if BLAKE2b-256 MAC initialization fails (should never happen with a 32-byte key).
#[must_use]
pub fn verify_auth_tag(key: &[u8; 32], data: &[u8], tag: &[u8]) -> bool {
    let mut mac = <Blake2bMac256 as KeyInit>::new_from_slice(key)
        .expect("BLAKE2bMac256 accepts 32-byte keys");
    mac.update(data);
    mac.verify_slice(tag).is_ok()
}

// ---- Message building / parsing helpers ------------------------------------

/// Encode a `SocketAddrV4` into 6 bytes (4 IP + 2 port, big-endian).
#[must_use]
pub fn encode_addr_v4(addr: SocketAddrV4) -> [u8; PEER_ADDR_LEN] {
    let mut buf = [0u8; PEER_ADDR_LEN];
    buf[..4].copy_from_slice(&addr.ip().octets());
    buf[4..6].copy_from_slice(&addr.port().to_be_bytes());
    buf
}

/// Decode a `SocketAddrV4` from 6 bytes (4 IP + 2 port, big-endian).
#[must_use]
pub fn decode_addr_v4(buf: &[u8]) -> Option<SocketAddrV4> {
    if buf.len() < PEER_ADDR_LEN {
        return None;
    }
    let ip = Ipv4Addr::new(buf[0], buf[1], buf[2], buf[3]);
    let port = u16::from_be_bytes([buf[4], buf[5]]);
    Some(SocketAddrV4::new(ip, port))
}

/// Build an authenticated control message.
///
/// Layout: `[type: 1] [payload_len: 2] [payload] [auth_tag: 32]`
///
/// The auth tag covers `[type + payload_len + payload]`.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn build_control_msg(msg_type: MsgType, payload: &[u8], key: &[u8; 32]) -> Vec<u8> {
    let payload_len = payload.len() as u16;
    let total = HEADER_LEN + payload.len() + AUTH_TAG_LEN;
    let mut buf = Vec::with_capacity(total);
    buf.push(msg_type as u8);
    buf.extend_from_slice(&payload_len.to_be_bytes());
    buf.extend_from_slice(payload);

    // Auth tag covers everything before the tag itself
    let tag = compute_auth_tag(key, &buf);
    buf.extend_from_slice(&tag);
    buf
}

/// Build a DATA message (no auth tag needed -- relay already authenticated).
///
/// Layout: `[0x80: 1] [length: 2] [peer_addr: 6] [raw_data]`
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn build_data_msg(peer_addr: SocketAddrV4, data: &[u8]) -> Vec<u8> {
    let inner_len = (PEER_ADDR_LEN + data.len()) as u16;
    let total = HEADER_LEN + PEER_ADDR_LEN + data.len();
    let mut buf = Vec::with_capacity(total);
    buf.push(MsgType::Data as u8);
    buf.extend_from_slice(&inner_len.to_be_bytes());
    buf.extend_from_slice(&encode_addr_v4(peer_addr));
    buf.extend_from_slice(data);
    buf
}

/// Parse a relay protocol message header. Returns (type, payload).
///
/// For control messages, the payload includes the auth tag at the end.
/// For data messages, the payload includes the peer addr + raw data.
#[must_use]
pub fn parse_msg(buf: &[u8]) -> Option<(MsgType, &[u8])> {
    if buf.len() < HEADER_LEN {
        return None;
    }
    let msg_type = MsgType::from_byte(buf[0])?;
    let payload_len = u16::from_be_bytes([buf[1], buf[2]]) as usize;
    let expected = HEADER_LEN + payload_len;

    if msg_type == MsgType::Data {
        // Data messages have no auth tag
        if buf.len() < expected {
            return None;
        }
        Some((msg_type, &buf[HEADER_LEN..expected]))
    } else {
        // Control messages have a 32-byte auth tag after the payload
        let expected_with_tag = expected + AUTH_TAG_LEN;
        if buf.len() < expected_with_tag {
            return None;
        }
        Some((msg_type, &buf[HEADER_LEN..expected_with_tag]))
    }
}

/// Parse and verify a control message. Returns the payload (without auth tag).
#[must_use]
pub fn parse_and_verify_control(buf: &[u8], key: &[u8; 32]) -> Option<(MsgType, Vec<u8>)> {
    if buf.len() < HEADER_LEN {
        return None;
    }
    let msg_type = MsgType::from_byte(buf[0])?;
    if msg_type == MsgType::Data {
        return None; // Data messages are not control messages
    }
    let payload_len = u16::from_be_bytes([buf[1], buf[2]]) as usize;
    let msg_end = HEADER_LEN + payload_len;
    let total_end = msg_end + AUTH_TAG_LEN;

    if buf.len() < total_end {
        return None;
    }

    let msg_bytes = &buf[..msg_end];
    let tag = &buf[msg_end..total_end];

    if !verify_auth_tag(key, msg_bytes, tag) {
        return None;
    }

    Some((msg_type, buf[HEADER_LEN..msg_end].to_vec()))
}

/// Parse a DATA message payload into `(peer_addr, raw_data)`.
#[must_use]
pub fn parse_data_payload(payload: &[u8]) -> Option<(SocketAddrV4, &[u8])> {
    if payload.len() < PEER_ADDR_LEN {
        return None;
    }
    let addr = decode_addr_v4(&payload[..PEER_ADDR_LEN])?;
    Some((addr, &payload[PEER_ADDR_LEN..]))
}

// ---- Relay allocation -------------------------------------------------------

/// Active relay allocation state.
#[allow(clippy::struct_field_names)]
struct RelayAllocation {
    /// The address the relay server allocated for us.
    relay_addr: SocketAddr,
    /// Server-assigned allocation ID (16 bytes).
    allocation_id: [u8; 16],
    /// When this allocation expires.
    expires_at: Instant,
    /// Allocation lifetime in seconds.
    lifetime_secs: u32,
}

// ---- Relay client -----------------------------------------------------------

/// Client for the `ZLayer` custom relay protocol.
///
/// Connects to a relay server, allocates a relay address, and runs a local
/// UDP proxy that bridges `WireGuard` traffic through the relay.
pub struct RelayClient {
    server_addr: SocketAddr,
    username: String,
    auth_key: [u8; 32],
    allocation: Option<RelayAllocation>,
    socket: Option<Arc<UdpSocket>>,
    local_proxy_addr: Option<SocketAddr>,
    proxy_handle: Option<tokio::task::JoinHandle<()>>,
}

impl RelayClient {
    /// Create a new relay client from a turn server config.
    ///
    /// # Errors
    ///
    /// Returns [`OverlayError::TurnRelay`] if the server address cannot be parsed.
    pub fn new(config: &TurnServerConfig) -> Result<Self, OverlayError> {
        let server_addr: SocketAddr = config
            .address
            .parse()
            .map_err(|e| OverlayError::TurnRelay(format!("Invalid relay server address: {e}")))?;

        let auth_key = derive_auth_key(&config.credential);

        Ok(Self {
            server_addr,
            username: config.username.clone(),
            auth_key,
            allocation: None,
            socket: None,
            local_proxy_addr: None,
            proxy_handle: None,
        })
    }

    /// Allocate a relay address from the server.
    ///
    /// Sends an `AllocateReq` with the username, receives an `AllocateResp`
    /// containing the relay address and allocation ID.
    ///
    /// # Errors
    ///
    /// Returns [`OverlayError::TurnRelay`] on network or protocol errors.
    pub async fn allocate(&mut self) -> Result<SocketAddr, OverlayError> {
        // Bind a UDP socket for communication with the relay server
        let socket = UdpSocket::bind("0.0.0.0:0")
            .await
            .map_err(|e| OverlayError::TurnRelay(format!("Failed to bind relay socket: {e}")))?;

        // Build AllocateReq payload: [username_len: 1] [username: N]
        let username_bytes = self.username.as_bytes();
        let mut payload = Vec::with_capacity(1 + username_bytes.len());
        payload.push(
            username_bytes
                .len()
                .try_into()
                .map_err(|_| OverlayError::TurnRelay("Username too long".to_string()))?,
        );
        payload.extend_from_slice(username_bytes);

        let msg = build_control_msg(MsgType::AllocateReq, &payload, &self.auth_key);
        socket
            .send_to(&msg, self.server_addr)
            .await
            .map_err(|e| OverlayError::TurnRelay(format!("Failed to send AllocateReq: {e}")))?;

        // Wait for response (5 second timeout)
        let mut buf = [0u8; 1024];
        let n = tokio::time::timeout(std::time::Duration::from_secs(5), socket.recv(&mut buf))
            .await
            .map_err(|_| OverlayError::TurnRelay("AllocateReq timed out".to_string()))?
            .map_err(|e| OverlayError::TurnRelay(format!("Failed to recv AllocateResp: {e}")))?;

        let (msg_type, resp_payload) = parse_and_verify_control(&buf[..n], &self.auth_key)
            .ok_or_else(|| {
                OverlayError::TurnRelay("Invalid or unauthenticated AllocateResp".to_string())
            })?;

        match msg_type {
            MsgType::AllocateResp => {
                // Payload: [relay_addr: 6] [allocation_id: 16] [lifetime: 4]
                if resp_payload.len() < PEER_ADDR_LEN + 16 + 4 {
                    return Err(OverlayError::TurnRelay(
                        "AllocateResp payload too short".to_string(),
                    ));
                }

                let relay_sockaddr =
                    decode_addr_v4(&resp_payload[..PEER_ADDR_LEN]).ok_or_else(|| {
                        OverlayError::TurnRelay("Failed to decode relay address".to_string())
                    })?;

                let mut allocation_id = [0u8; 16];
                allocation_id.copy_from_slice(&resp_payload[PEER_ADDR_LEN..PEER_ADDR_LEN + 16]);

                let lifetime_secs = u32::from_be_bytes([
                    resp_payload[PEER_ADDR_LEN + 16],
                    resp_payload[PEER_ADDR_LEN + 17],
                    resp_payload[PEER_ADDR_LEN + 18],
                    resp_payload[PEER_ADDR_LEN + 19],
                ]);

                let relay_addr = SocketAddr::V4(relay_sockaddr);

                debug!(
                    relay_addr = %relay_addr,
                    lifetime = lifetime_secs,
                    "Relay allocation succeeded"
                );

                self.allocation = Some(RelayAllocation {
                    relay_addr,
                    allocation_id,
                    expires_at: Instant::now()
                        + std::time::Duration::from_secs(u64::from(lifetime_secs)),
                    lifetime_secs,
                });

                self.socket = Some(Arc::new(socket));
                Ok(relay_addr)
            }
            MsgType::AllocateErr => {
                let err_msg = String::from_utf8_lossy(&resp_payload);
                Err(OverlayError::TurnRelay(format!(
                    "Allocation rejected: {err_msg}"
                )))
            }
            other => Err(OverlayError::TurnRelay(format!(
                "Unexpected response type: {other:?}"
            ))),
        }
    }

    /// Create a permission for a peer address on the relay server.
    ///
    /// The relay will only forward data to/from peers with active permissions.
    ///
    /// # Errors
    ///
    /// Returns [`OverlayError::TurnRelay`] if no allocation is active or
    /// the server rejects the permission.
    pub async fn create_permission(&mut self, peer_addr: SocketAddr) -> Result<(), OverlayError> {
        let allocation = self
            .allocation
            .as_ref()
            .ok_or_else(|| OverlayError::TurnRelay("No active allocation".to_string()))?;

        let socket = self
            .socket
            .as_ref()
            .ok_or_else(|| OverlayError::TurnRelay("No relay socket".to_string()))?;

        let peer_v4 = match peer_addr {
            SocketAddr::V4(v4) => v4,
            SocketAddr::V6(_) => {
                return Err(OverlayError::TurnRelay(
                    "IPv6 peer addresses not supported".to_string(),
                ));
            }
        };

        // Payload: [allocation_id: 16] [peer_addr: 6]
        let mut payload = Vec::with_capacity(16 + PEER_ADDR_LEN);
        payload.extend_from_slice(&allocation.allocation_id);
        payload.extend_from_slice(&encode_addr_v4(peer_v4));

        let msg = build_control_msg(MsgType::PermissionReq, &payload, &self.auth_key);
        socket
            .send_to(&msg, self.server_addr)
            .await
            .map_err(|e| OverlayError::TurnRelay(format!("Failed to send PermissionReq: {e}")))?;

        // Wait for response
        let mut buf = [0u8; 512];
        let n = tokio::time::timeout(std::time::Duration::from_secs(5), socket.recv(&mut buf))
            .await
            .map_err(|_| OverlayError::TurnRelay("PermissionReq timed out".to_string()))?
            .map_err(|e| OverlayError::TurnRelay(format!("Failed to recv PermissionResp: {e}")))?;

        let (msg_type, _) = parse_and_verify_control(&buf[..n], &self.auth_key)
            .ok_or_else(|| OverlayError::TurnRelay("Invalid PermissionResp".to_string()))?;

        if msg_type != MsgType::PermissionResp {
            return Err(OverlayError::TurnRelay(format!(
                "Expected PermissionResp, got {msg_type:?}"
            )));
        }

        debug!(peer = %peer_addr, "Relay permission created");
        Ok(())
    }

    /// Start a local UDP proxy that bridges `WireGuard` traffic through the relay.
    ///
    /// Returns the local proxy address. Set the `WireGuard` peer endpoint to this
    /// address so WG sends packets to the proxy, which forwards them through
    /// the relay to the remote peer.
    ///
    /// # Errors
    ///
    /// Returns [`OverlayError::TurnRelay`] if no allocation is active.
    pub async fn start_proxy(&mut self, wg_port: u16) -> Result<SocketAddr, OverlayError> {
        let relay_socket = self
            .socket
            .as_ref()
            .ok_or_else(|| OverlayError::TurnRelay("No relay socket".to_string()))?
            .clone();

        let allocation = self
            .allocation
            .as_ref()
            .ok_or_else(|| OverlayError::TurnRelay("No active allocation".to_string()))?;

        let relay_addr = allocation.relay_addr;
        let server_addr = self.server_addr;

        // Bind local proxy on loopback with ephemeral port
        let proxy_socket =
            Arc::new(UdpSocket::bind("127.0.0.1:0").await.map_err(|e| {
                OverlayError::TurnRelay(format!("Failed to bind proxy socket: {e}"))
            })?);
        let proxy_addr = proxy_socket
            .local_addr()
            .map_err(|e| OverlayError::TurnRelay(format!("Failed to get proxy addr: {e}")))?;

        let wg_local = SocketAddrV4::new(Ipv4Addr::LOCALHOST, wg_port);

        debug!(
            proxy_addr = %proxy_addr,
            relay_addr = %relay_addr,
            "Starting relay proxy"
        );

        // Spawn proxy task: bidirectional forwarding
        // Two separate buffers to avoid double mutable borrow in tokio::select!
        let proxy_read = proxy_socket.clone();
        let relay_read = relay_socket.clone();

        let handle = tokio::spawn(async move {
            #[allow(clippy::large_stack_arrays)]
            let mut proxy_buf = [0u8; 65536];
            #[allow(clippy::large_stack_arrays)]
            let mut relay_buf = [0u8; 65536];

            loop {
                tokio::select! {
                    // WG -> proxy -> relay server (wrap as DATA)
                    result = proxy_read.recv_from(&mut proxy_buf) => {
                        match result {
                            Ok((n, _from)) => {
                                // Destination is the relay_addr (the peer's relay allocation)
                                let peer_v4 = match relay_addr {
                                    SocketAddr::V4(v4) => v4,
                                    SocketAddr::V6(_) => continue,
                                };
                                let data_msg = build_data_msg(peer_v4, &proxy_buf[..n]);
                                if let Err(e) = relay_read.send_to(&data_msg, server_addr).await {
                                    warn!(error = %e, "Failed to send data through relay");
                                }
                            }
                            Err(e) => {
                                warn!(error = %e, "Proxy recv error");
                                break;
                            }
                        }
                    }
                    // Relay server -> proxy -> WG
                    result = relay_read.recv_from(&mut relay_buf) => {
                        match result {
                            Ok((n, _from)) => {
                                if let Some(MsgType::Data) = relay_buf.first().and_then(|&b| MsgType::from_byte(b)) {
                                    if let Some((_, raw_data)) = parse_msg(&relay_buf[..n])
                                        .and_then(|(_, payload)| parse_data_payload(payload))
                                    {
                                        let wg_dest = SocketAddr::V4(wg_local);
                                        if let Err(e) = proxy_read.send_to(raw_data, wg_dest).await {
                                            warn!(error = %e, "Failed to forward to WG");
                                        }
                                    }
                                }
                                // Ignore non-DATA messages in proxy loop
                            }
                            Err(e) => {
                                warn!(error = %e, "Relay recv error");
                                break;
                            }
                        }
                    }
                }
            }
        });

        self.proxy_handle = Some(handle);
        self.local_proxy_addr = Some(proxy_addr);

        Ok(proxy_addr)
    }

    /// Refresh the relay allocation before it expires.
    ///
    /// # Errors
    ///
    /// Returns [`OverlayError::TurnRelay`] if no allocation exists or refresh fails.
    pub async fn refresh(&mut self) -> Result<(), OverlayError> {
        let allocation = self
            .allocation
            .as_ref()
            .ok_or_else(|| OverlayError::TurnRelay("No active allocation".to_string()))?;

        let socket = self
            .socket
            .as_ref()
            .ok_or_else(|| OverlayError::TurnRelay("No relay socket".to_string()))?;

        // Payload: [allocation_id: 16] [lifetime: 4]
        let mut payload = Vec::with_capacity(20);
        payload.extend_from_slice(&allocation.allocation_id);
        payload.extend_from_slice(&allocation.lifetime_secs.to_be_bytes());

        let msg = build_control_msg(MsgType::RefreshReq, &payload, &self.auth_key);
        socket
            .send_to(&msg, self.server_addr)
            .await
            .map_err(|e| OverlayError::TurnRelay(format!("Failed to send RefreshReq: {e}")))?;

        // Wait for response
        let mut buf = [0u8; 512];
        let n = tokio::time::timeout(std::time::Duration::from_secs(5), socket.recv(&mut buf))
            .await
            .map_err(|_| OverlayError::TurnRelay("RefreshReq timed out".to_string()))?
            .map_err(|e| OverlayError::TurnRelay(format!("Failed to recv RefreshResp: {e}")))?;

        let (msg_type, resp_payload) = parse_and_verify_control(&buf[..n], &self.auth_key)
            .ok_or_else(|| OverlayError::TurnRelay("Invalid RefreshResp".to_string()))?;

        if msg_type != MsgType::RefreshResp {
            return Err(OverlayError::TurnRelay(format!(
                "Expected RefreshResp, got {msg_type:?}"
            )));
        }

        // Parse new lifetime from response
        if resp_payload.len() >= 4 {
            let new_lifetime = u32::from_be_bytes([
                resp_payload[0],
                resp_payload[1],
                resp_payload[2],
                resp_payload[3],
            ]);

            if let Some(ref mut alloc) = self.allocation {
                alloc.lifetime_secs = new_lifetime;
                alloc.expires_at =
                    Instant::now() + std::time::Duration::from_secs(u64::from(new_lifetime));
            }

            debug!(lifetime = new_lifetime, "Relay allocation refreshed");
        }

        Ok(())
    }

    /// Deallocate the relay address.
    ///
    /// # Errors
    ///
    /// Returns [`OverlayError::TurnRelay`] if no allocation exists or deallocation fails.
    pub async fn deallocate(&mut self) -> Result<(), OverlayError> {
        let allocation = self
            .allocation
            .as_ref()
            .ok_or_else(|| OverlayError::TurnRelay("No active allocation".to_string()))?;

        let socket = self
            .socket
            .as_ref()
            .ok_or_else(|| OverlayError::TurnRelay("No relay socket".to_string()))?;

        // Payload: [allocation_id: 16]
        let msg = build_control_msg(
            MsgType::Deallocate,
            &allocation.allocation_id,
            &self.auth_key,
        );
        // Best-effort send, don't block on response
        let _ = socket.send_to(&msg, self.server_addr).await;

        // Clean up
        if let Some(handle) = self.proxy_handle.take() {
            handle.abort();
        }
        self.allocation = None;
        self.local_proxy_addr = None;

        debug!("Relay allocation deallocated");
        Ok(())
    }

    /// Get the local proxy address (what WG peer endpoint should be set to).
    #[must_use]
    pub fn proxy_addr(&self) -> Option<SocketAddr> {
        self.local_proxy_addr
    }

    /// Check if the allocation is active (exists and has not expired).
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.allocation
            .as_ref()
            .is_some_and(|a| Instant::now() < a.expires_at)
    }

    /// Build a Relay candidate using the local proxy address.
    ///
    /// Returns `None` if no proxy is running.
    #[must_use]
    pub fn candidate(&self) -> Option<Candidate> {
        self.local_proxy_addr
            .map(|addr| Candidate::new(CandidateType::Relay, addr))
    }
}

impl Drop for RelayClient {
    fn drop(&mut self) {
        if let Some(handle) = self.proxy_handle.take() {
            handle.abort();
        }
    }
}

// ---- Tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddrV4};

    #[test]
    fn test_derive_auth_key() {
        let key1 = derive_auth_key("my_secret");
        let key2 = derive_auth_key("my_secret");
        assert_eq!(key1, key2, "Same credential must produce same key");

        let key3 = derive_auth_key("different_secret");
        assert_ne!(
            key1, key3,
            "Different credentials must produce different keys"
        );
    }

    #[test]
    fn test_auth_tag_roundtrip() {
        let key = derive_auth_key("test_credential");
        let data = b"hello relay world";
        let tag = compute_auth_tag(&key, data);

        assert!(verify_auth_tag(&key, data, &tag), "Tag should verify");
        assert!(
            !verify_auth_tag(&key, b"wrong data", &tag),
            "Tag should fail with wrong data"
        );

        let wrong_key = derive_auth_key("wrong_credential");
        assert!(
            !verify_auth_tag(&wrong_key, data, &tag),
            "Tag should fail with wrong key"
        );
    }

    #[test]
    fn test_encode_decode_addr_v4() {
        let addr = SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 100), 51820);
        let encoded = encode_addr_v4(addr);
        let decoded = decode_addr_v4(&encoded).unwrap();
        assert_eq!(addr, decoded);
    }

    #[test]
    fn test_encode_decode_addr_v4_edge_cases() {
        // Broadcast
        let addr = SocketAddrV4::new(Ipv4Addr::BROADCAST, 65535);
        let encoded = encode_addr_v4(addr);
        let decoded = decode_addr_v4(&encoded).unwrap();
        assert_eq!(addr, decoded);

        // All zeros
        let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
        let encoded = encode_addr_v4(addr);
        let decoded = decode_addr_v4(&encoded).unwrap();
        assert_eq!(addr, decoded);
    }

    #[test]
    fn test_decode_addr_v4_too_short() {
        assert!(decode_addr_v4(&[1, 2, 3]).is_none());
    }

    #[test]
    fn test_build_and_parse_control_msg() {
        let key = derive_auth_key("test");
        let payload = b"test_payload";
        let msg = build_control_msg(MsgType::AllocateReq, payload, &key);

        // Should parse and verify
        let (msg_type, parsed_payload) = parse_and_verify_control(&msg, &key).unwrap();
        assert_eq!(msg_type, MsgType::AllocateReq);
        assert_eq!(&parsed_payload, payload);

        // Should fail with wrong key
        let wrong_key = derive_auth_key("wrong");
        assert!(parse_and_verify_control(&msg, &wrong_key).is_none());
    }

    #[test]
    fn test_build_and_parse_data_msg() {
        let peer_addr = SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 1), 3478);
        let raw_data = b"wireguard_packet_data";
        let msg = build_data_msg(peer_addr, raw_data);

        let (msg_type, payload) = parse_msg(&msg).unwrap();
        assert_eq!(msg_type, MsgType::Data);

        let (parsed_addr, parsed_data) = parse_data_payload(payload).unwrap();
        assert_eq!(parsed_addr, peer_addr);
        assert_eq!(parsed_data, raw_data);
    }

    #[test]
    fn test_msg_type_from_byte() {
        assert_eq!(MsgType::from_byte(0x01), Some(MsgType::AllocateReq));
        assert_eq!(MsgType::from_byte(0x02), Some(MsgType::AllocateResp));
        assert_eq!(MsgType::from_byte(0x03), Some(MsgType::AllocateErr));
        assert_eq!(MsgType::from_byte(0x04), Some(MsgType::PermissionReq));
        assert_eq!(MsgType::from_byte(0x05), Some(MsgType::PermissionResp));
        assert_eq!(MsgType::from_byte(0x06), Some(MsgType::RefreshReq));
        assert_eq!(MsgType::from_byte(0x07), Some(MsgType::RefreshResp));
        assert_eq!(MsgType::from_byte(0x08), Some(MsgType::Deallocate));
        assert_eq!(MsgType::from_byte(0x80), Some(MsgType::Data));
        assert_eq!(MsgType::from_byte(0xFF), None);
    }

    #[test]
    fn test_parse_msg_too_short() {
        assert!(parse_msg(&[]).is_none());
        assert!(parse_msg(&[0x01]).is_none());
        assert!(parse_msg(&[0x01, 0x00]).is_none());
    }

    #[test]
    fn test_relay_client_new() {
        let config = TurnServerConfig {
            address: "127.0.0.1:3478".to_string(),
            username: "testuser".to_string(),
            credential: "testpass".to_string(),
            region: None,
        };
        let client = RelayClient::new(&config).unwrap();
        assert_eq!(client.server_addr, "127.0.0.1:3478".parse().unwrap());
        assert_eq!(client.username, "testuser");
        assert!(!client.is_active());
        assert!(client.proxy_addr().is_none());
        assert!(client.candidate().is_none());
    }

    #[test]
    fn test_relay_client_invalid_address() {
        let config = TurnServerConfig {
            address: "not_a_valid_address".to_string(),
            username: "user".to_string(),
            credential: "pass".to_string(),
            region: None,
        };
        assert!(RelayClient::new(&config).is_err());
    }

    #[test]
    fn test_control_msg_all_types() {
        let key = derive_auth_key("k");
        for msg_type in [
            MsgType::AllocateReq,
            MsgType::AllocateResp,
            MsgType::AllocateErr,
            MsgType::PermissionReq,
            MsgType::PermissionResp,
            MsgType::RefreshReq,
            MsgType::RefreshResp,
            MsgType::Deallocate,
        ] {
            let msg = build_control_msg(msg_type, b"p", &key);
            let (parsed, payload) = parse_and_verify_control(&msg, &key).unwrap();
            assert_eq!(parsed, msg_type);
            assert_eq!(payload, b"p");
        }
    }

    #[test]
    fn test_allocation_id_in_control_msg() {
        let key = derive_auth_key("test");
        let allocation_id: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let msg = build_control_msg(MsgType::RefreshReq, &allocation_id, &key);

        let (_, payload) = parse_and_verify_control(&msg, &key).unwrap();
        assert_eq!(payload.len(), 16);
        assert_eq!(payload.as_slice(), &allocation_id);
    }
}
