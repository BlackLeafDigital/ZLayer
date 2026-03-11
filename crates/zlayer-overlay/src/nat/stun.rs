//! Minimal STUN client per RFC 5389
//!
//! Implements just enough of the STUN Binding protocol to discover our
//! server-reflexive (public) address and detect NAT behavior (endpoint-independent
//! vs symmetric). No external STUN crate required.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

use rand::Rng;
use socket2::{Domain, Protocol, Socket, Type};
use thiserror::Error;
use tokio::net::UdpSocket;

use super::config::StunServerConfig;

// ---- STUN protocol constants (RFC 5389) ------------------------------------

const MAGIC_COOKIE: u32 = 0x2112_A442;
const BINDING_REQUEST: u16 = 0x0001;
const BINDING_RESPONSE: u16 = 0x0101;
const ATTR_MAPPED_ADDRESS: u16 = 0x0001;
const ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;
const STUN_HEADER_SIZE: usize = 20;

// ---- Error type ------------------------------------------------------------

/// Errors specific to STUN operations.
#[derive(Debug, Error)]
pub enum StunError {
    /// Response too short to contain a valid STUN header.
    #[error("STUN response too short ({0} bytes, need at least {STUN_HEADER_SIZE})")]
    TooShort(usize),

    /// The response message type is not a Binding Response (0x0101).
    #[error("Expected Binding Response (0x0101), got 0x{0:04X}")]
    NotBindingResponse(u16),

    /// The magic cookie field does not match `0x2112A442`.
    #[error("Bad magic cookie in STUN response")]
    BadMagicCookie,

    /// The 12-byte transaction ID does not match what we sent.
    #[error("Transaction ID mismatch")]
    TransactionMismatch,

    /// Neither XOR-MAPPED-ADDRESS nor MAPPED-ADDRESS was found.
    #[error("No mapped address attribute in STUN response")]
    NoMappedAddress,

    /// An I/O error from the UDP socket.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Timed out waiting for a STUN response.
    #[error("STUN query timed out")]
    Timeout,

    /// DNS resolution of a STUN server hostname failed.
    #[error("DNS resolution failed for '{0}'")]
    DnsResolution(String),

    /// No STUN servers configured.
    #[error("No STUN servers configured")]
    NoServers,
}

// ---- Public types ----------------------------------------------------------

/// A reflexive (public) address discovered via a STUN server.
#[derive(Debug, Clone)]
pub struct ReflexiveAddress {
    /// The public address:port the STUN server observed.
    pub address: SocketAddr,
    /// Human-readable label of the STUN server that reported this.
    pub server: String,
}

/// Observed NAT mapping behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NatBehavior {
    /// All STUN servers saw the same reflexive port (or we only queried one).
    /// Hole punching is likely to succeed.
    EndpointIndependent,
    /// Different STUN servers saw different reflexive ports.
    /// Hole punching will probably fail; a relay is needed.
    Symmetric,
}

// ---- STUN client -----------------------------------------------------------

/// A lightweight STUN client that discovers reflexive addresses.
pub struct StunClient {
    servers: Vec<StunServerConfig>,
}

impl StunClient {
    /// Create a new STUN client with the given server list.
    #[must_use]
    pub fn new(servers: Vec<StunServerConfig>) -> Self {
        Self { servers }
    }

    /// Query a single STUN server for our reflexive address.
    ///
    /// Creates an ephemeral UDP socket, sends a Binding Request, and waits up
    /// to 3 seconds for a Binding Response.
    ///
    /// # Errors
    ///
    /// Returns [`StunError::Io`] on socket failures, [`StunError::Timeout`] if
    /// no response arrives within 3 seconds, or a parse error if the response
    /// is malformed.
    pub async fn query_server(
        &self,
        server_addr: SocketAddr,
        server_name: &str,
    ) -> Result<ReflexiveAddress, StunError> {
        // Create UDP socket via socket2 so we can bind to port 0 (ephemeral)
        let domain = if server_addr.is_ipv4() {
            Domain::IPV4
        } else {
            Domain::IPV6
        };
        let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))?;
        socket.set_nonblocking(true)?;

        let bind_addr: SocketAddr = if server_addr.is_ipv4() {
            SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0)
        } else {
            SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), 0)
        };
        socket.bind(&bind_addr.into())?;

        // Convert socket2::Socket -> std::net::UdpSocket -> tokio::net::UdpSocket
        let std_socket: std::net::UdpSocket = socket.into();
        let udp = UdpSocket::from_std(std_socket)?;

        // Build Binding Request
        let mut txn_id = [0u8; 12];
        rand::rng().fill(&mut txn_id);
        let request = build_binding_request(&txn_id);

        udp.send_to(&request, server_addr).await?;

        // Wait for response with 3-second timeout
        let mut buf = [0u8; 576]; // RFC 5389 recommended minimum
        let n = tokio::time::timeout(std::time::Duration::from_secs(3), udp.recv(&mut buf))
            .await
            .map_err(|_| StunError::Timeout)?
            .map_err(StunError::Io)?;

        let reflexive = parse_binding_response(&buf[..n], &txn_id)?;

        tracing::debug!(
            server = %server_name,
            reflexive = %reflexive,
            "STUN query succeeded"
        );

        Ok(ReflexiveAddress {
            address: reflexive,
            server: server_name.to_string(),
        })
    }

    /// Query all configured STUN servers in parallel.
    ///
    /// Returns all successfully discovered reflexive addresses and the inferred
    /// NAT behavior. If all servers fail, returns the last error.
    ///
    /// # Errors
    ///
    /// Returns [`StunError::NoServers`] if no servers are configured, or the
    /// last per-server error if every query fails.
    pub async fn discover(&self) -> Result<(Vec<ReflexiveAddress>, NatBehavior), StunError> {
        if self.servers.is_empty() {
            return Err(StunError::NoServers);
        }

        let mut set = tokio::task::JoinSet::new();

        for server_cfg in &self.servers {
            let address_str = server_cfg.address.clone();
            let label = server_cfg
                .label
                .clone()
                .unwrap_or_else(|| server_cfg.address.clone());

            // Resolve hostname -> SocketAddr
            let resolved = match tokio::net::lookup_host(&address_str).await {
                Ok(mut addrs) => {
                    if let Some(addr) = addrs.next() {
                        addr
                    } else {
                        tracing::warn!(server = %address_str, "DNS returned no addresses");
                        continue;
                    }
                }
                Err(e) => {
                    tracing::warn!(server = %address_str, error = %e, "DNS resolution failed");
                    continue;
                }
            };

            // We need a fresh client reference per task; clone the necessary data
            let label_clone = label.clone();
            // Build a minimal single-server client for the spawned task
            let servers_clone = vec![StunServerConfig {
                address: address_str,
                label: Some(label_clone.clone()),
            }];

            set.spawn(async move {
                let client = StunClient::new(servers_clone);
                client.query_server(resolved, &label_clone).await
            });
        }

        let mut results: Vec<ReflexiveAddress> = Vec::new();
        let mut last_error: Option<StunError> = None;

        while let Some(join_result) = set.join_next().await {
            match join_result {
                Ok(Ok(reflexive)) => results.push(reflexive),
                Ok(Err(e)) => {
                    tracing::debug!(error = %e, "STUN query failed");
                    last_error = Some(e);
                }
                Err(e) => {
                    tracing::debug!(error = %e, "STUN task panicked");
                }
            }
        }

        if results.is_empty() {
            return Err(last_error.unwrap_or(StunError::NoServers));
        }

        let behavior = detect_nat_behavior(&results);

        tracing::debug!(
            count = results.len(),
            behavior = ?behavior,
            "STUN discovery complete"
        );

        Ok((results, behavior))
    }
}

// ---- Protocol helpers (free functions) -------------------------------------

/// Build a 20-byte STUN Binding Request.
///
/// Layout (RFC 5389 Section 6):
///   - 2 bytes: message type (0x0001 = Binding Request)
///   - 2 bytes: message length (0 = no attributes)
///   - 4 bytes: magic cookie (0x2112A442)
///   - 12 bytes: transaction ID
fn build_binding_request(transaction_id: &[u8; 12]) -> [u8; 20] {
    let mut msg = [0u8; STUN_HEADER_SIZE];
    // Message type: Binding Request
    msg[0..2].copy_from_slice(&BINDING_REQUEST.to_be_bytes());
    // Message length: 0 (no attributes in the request)
    msg[2..4].copy_from_slice(&0u16.to_be_bytes());
    // Magic cookie
    msg[4..8].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
    // Transaction ID
    msg[8..20].copy_from_slice(transaction_id);
    msg
}

/// Parse a STUN Binding Response and extract the reflexive address.
///
/// Validates the header (message type, magic cookie, transaction ID) then
/// searches for XOR-MAPPED-ADDRESS (preferred) or MAPPED-ADDRESS (fallback).
fn parse_binding_response(
    data: &[u8],
    expected_txn_id: &[u8; 12],
) -> Result<SocketAddr, StunError> {
    if data.len() < STUN_HEADER_SIZE {
        return Err(StunError::TooShort(data.len()));
    }

    // Message type
    let msg_type = u16::from_be_bytes([data[0], data[1]]);
    if msg_type != BINDING_RESPONSE {
        return Err(StunError::NotBindingResponse(msg_type));
    }

    // Attributes length (bytes after the 20-byte header)
    let attr_len = u16::from_be_bytes([data[2], data[3]]) as usize;

    // Magic cookie
    let cookie = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    if cookie != MAGIC_COOKIE {
        return Err(StunError::BadMagicCookie);
    }

    // Transaction ID
    if data[8..20] != *expected_txn_id {
        return Err(StunError::TransactionMismatch);
    }

    // Walk attributes
    let attr_end = STUN_HEADER_SIZE + attr_len.min(data.len() - STUN_HEADER_SIZE);
    let mut offset = STUN_HEADER_SIZE;
    let mut xor_addr: Option<SocketAddr> = None;
    let mut mapped_addr: Option<SocketAddr> = None;

    while offset + 4 <= attr_end {
        let attr_type = u16::from_be_bytes([data[offset], data[offset + 1]]);
        let value_len = u16::from_be_bytes([data[offset + 2], data[offset + 3]]) as usize;
        let value_start = offset + 4;
        let value_end = value_start + value_len;

        if value_end > data.len() {
            break;
        }

        let value = &data[value_start..value_end];

        match attr_type {
            ATTR_XOR_MAPPED_ADDRESS => {
                xor_addr = parse_xor_mapped_address(value, expected_txn_id);
            }
            ATTR_MAPPED_ADDRESS => {
                mapped_addr = parse_mapped_address(value);
            }
            _ => {}
        }

        // Attributes are padded to 4-byte boundaries
        let padded = (value_len + 3) & !3;
        offset = value_start + padded;
    }

    xor_addr.or(mapped_addr).ok_or(StunError::NoMappedAddress)
}

/// Decode an XOR-MAPPED-ADDRESS attribute value (RFC 5389 Section 15.2).
///
/// The port is XOR-decoded with the high 16 bits of the magic cookie (0x2112).
/// IPv4 addresses are XOR-decoded with the full magic cookie (0x2112A442).
fn parse_xor_mapped_address(value: &[u8], txn_id: &[u8; 12]) -> Option<SocketAddr> {
    if value.len() < 8 {
        return None;
    }

    // Byte 0 is reserved (0x00), byte 1 is address family
    let family = value[1];
    let xored_port = u16::from_be_bytes([value[2], value[3]]);
    let port = xored_port ^ (MAGIC_COOKIE >> 16) as u16;

    match family {
        0x01 => {
            // IPv4: 4 bytes XOR-decoded with magic cookie
            if value.len() < 8 {
                return None;
            }
            let xored_ip = u32::from_be_bytes([value[4], value[5], value[6], value[7]]);
            let ip = Ipv4Addr::from(xored_ip ^ MAGIC_COOKIE);
            Some(SocketAddr::new(ip.into(), port))
        }
        0x02 => {
            // IPv6: 16 bytes XOR-decoded with magic cookie + transaction ID
            if value.len() < 20 {
                return None;
            }
            let mut ip_bytes = [0u8; 16];
            ip_bytes.copy_from_slice(&value[4..20]);
            let cookie_bytes = MAGIC_COOKIE.to_be_bytes();
            for (i, b) in ip_bytes.iter_mut().enumerate() {
                if i < 4 {
                    *b ^= cookie_bytes[i];
                } else {
                    *b ^= txn_id[i - 4];
                }
            }
            let ip = Ipv6Addr::from(ip_bytes);
            Some(SocketAddr::new(ip.into(), port))
        }
        _ => None,
    }
}

/// Decode a MAPPED-ADDRESS attribute value (RFC 5389 Section 15.1).
///
/// Legacy format without XOR obfuscation. Used as fallback for older servers.
fn parse_mapped_address(value: &[u8]) -> Option<SocketAddr> {
    if value.len() < 8 {
        return None;
    }

    let family = value[1];
    let port = u16::from_be_bytes([value[2], value[3]]);

    match family {
        0x01 => {
            // IPv4
            let ip = Ipv4Addr::new(value[4], value[5], value[6], value[7]);
            Some(SocketAddr::new(ip.into(), port))
        }
        0x02 => {
            // IPv6
            if value.len() < 20 {
                return None;
            }
            let mut ip_bytes = [0u8; 16];
            ip_bytes.copy_from_slice(&value[4..20]);
            let ip = Ipv6Addr::from(ip_bytes);
            Some(SocketAddr::new(ip.into(), port))
        }
        _ => None,
    }
}

/// Determine NAT behavior from multiple reflexive address results.
///
/// If all servers report the same reflexive port, the NAT is likely
/// endpoint-independent (good for hole punching). Different ports
/// indicate symmetric NAT (relay needed).
fn detect_nat_behavior(results: &[ReflexiveAddress]) -> NatBehavior {
    if results.len() <= 1 {
        return NatBehavior::EndpointIndependent;
    }

    let first_port = results[0].address.port();
    let all_same = results.iter().all(|r| r.address.port() == first_port);

    if all_same {
        NatBehavior::EndpointIndependent
    } else {
        NatBehavior::Symmetric
    }
}

// ---- Tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

    #[test]
    fn test_build_binding_request_header() {
        let txn_id = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let msg = build_binding_request(&txn_id);

        assert_eq!(
            msg.len(),
            20,
            "STUN Binding Request must be exactly 20 bytes"
        );

        // Message type: 0x0001
        assert_eq!(msg[0], 0x00);
        assert_eq!(msg[1], 0x01);

        // Message length: 0x0000
        assert_eq!(msg[2], 0x00);
        assert_eq!(msg[3], 0x00);

        // Magic cookie: 0x2112A442
        assert_eq!(msg[4], 0x21);
        assert_eq!(msg[5], 0x12);
        assert_eq!(msg[6], 0xA4);
        assert_eq!(msg[7], 0x42);

        // Transaction ID
        assert_eq!(&msg[8..20], &txn_id);
    }

    #[test]
    fn test_parse_binding_response_rfc5769_ipv4() {
        // Construct a synthetic Binding Response with XOR-MAPPED-ADDRESS
        // Reflexive address: 192.0.2.1:32853 (0xC000_0201, port 0x8055)
        let txn_id: [u8; 12] = [
            0xB7, 0xE7, 0xA7, 0x01, 0xBC, 0x34, 0xD6, 0x86, 0xFA, 0x87, 0xDF, 0xAE,
        ];

        let ip = Ipv4Addr::new(192, 0, 2, 1);
        let port: u16 = 32853;

        // XOR the port and IP
        let xored_port = port ^ (MAGIC_COOKIE >> 16) as u16;
        let xored_ip = u32::from_be_bytes(ip.octets()) ^ MAGIC_COOKIE;

        // Build XOR-MAPPED-ADDRESS attribute value
        let mut attr_value = [0u8; 8];
        attr_value[0] = 0x00; // reserved
        attr_value[1] = 0x01; // IPv4
        attr_value[2..4].copy_from_slice(&xored_port.to_be_bytes());
        attr_value[4..8].copy_from_slice(&xored_ip.to_be_bytes());

        // Build complete response
        let attr_header_len: u16 = 4 + 8; // type(2) + length(2) + value(8)
        let mut response = Vec::with_capacity(STUN_HEADER_SIZE + attr_header_len as usize);

        // Header
        response.extend_from_slice(&BINDING_RESPONSE.to_be_bytes());
        response.extend_from_slice(&attr_header_len.to_be_bytes());
        response.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
        response.extend_from_slice(&txn_id);

        // XOR-MAPPED-ADDRESS attribute
        response.extend_from_slice(&ATTR_XOR_MAPPED_ADDRESS.to_be_bytes());
        response.extend_from_slice(&8u16.to_be_bytes()); // value length
        response.extend_from_slice(&attr_value);

        let result = parse_binding_response(&response, &txn_id).unwrap();
        assert_eq!(result, SocketAddr::new(ip.into(), port));
    }

    #[test]
    fn test_parse_xor_mapped_address_ipv4() {
        // Test reflexive address: 198.51.100.50:12345
        let ip = Ipv4Addr::new(198, 51, 100, 50);
        let port: u16 = 12345;
        let txn_id = [0u8; 12];

        let xored_port = port ^ (MAGIC_COOKIE >> 16) as u16;
        let xored_ip = u32::from_be_bytes(ip.octets()) ^ MAGIC_COOKIE;

        let mut value = [0u8; 8];
        value[0] = 0x00;
        value[1] = 0x01; // IPv4
        value[2..4].copy_from_slice(&xored_port.to_be_bytes());
        value[4..8].copy_from_slice(&xored_ip.to_be_bytes());

        let result = parse_xor_mapped_address(&value, &txn_id).unwrap();
        assert_eq!(result.ip(), ip);
        assert_eq!(result.port(), port);
    }

    #[test]
    fn test_parse_mapped_address_ipv4() {
        let ip = Ipv4Addr::new(10, 0, 0, 1);
        let port: u16 = 51820;

        let mut value = [0u8; 8];
        value[0] = 0x00;
        value[1] = 0x01; // IPv4
        value[2..4].copy_from_slice(&port.to_be_bytes());
        value[4] = 10;
        value[5] = 0;
        value[6] = 0;
        value[7] = 1;

        let result = parse_mapped_address(&value).unwrap();
        assert_eq!(result, SocketAddr::new(ip.into(), port));
    }

    #[test]
    fn test_parse_binding_response_too_short() {
        let data = [0u8; 10];
        let txn_id = [0u8; 12];
        let err = parse_binding_response(&data, &txn_id).unwrap_err();
        assert!(matches!(err, StunError::TooShort(10)));
    }

    #[test]
    fn test_parse_binding_response_wrong_type() {
        let txn_id = [0u8; 12];
        let mut data = [0u8; 20];
        // Set message type to Binding Request instead of Response
        data[0..2].copy_from_slice(&BINDING_REQUEST.to_be_bytes());
        data[4..8].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
        data[8..20].copy_from_slice(&txn_id);

        let err = parse_binding_response(&data, &txn_id).unwrap_err();
        assert!(matches!(err, StunError::NotBindingResponse(0x0001)));
    }

    #[test]
    fn test_parse_binding_response_bad_cookie() {
        let txn_id = [0u8; 12];
        let mut data = [0u8; 20];
        data[0..2].copy_from_slice(&BINDING_RESPONSE.to_be_bytes());
        data[4..8].copy_from_slice(&0xDEAD_BEEFu32.to_be_bytes());
        data[8..20].copy_from_slice(&txn_id);

        let err = parse_binding_response(&data, &txn_id).unwrap_err();
        assert!(matches!(err, StunError::BadMagicCookie));
    }

    #[test]
    fn test_parse_binding_response_txn_mismatch() {
        let txn_id = [1u8; 12];
        let wrong_txn = [2u8; 12];
        let mut data = [0u8; 20];
        data[0..2].copy_from_slice(&BINDING_RESPONSE.to_be_bytes());
        data[4..8].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
        data[8..20].copy_from_slice(&wrong_txn);

        let err = parse_binding_response(&data, &txn_id).unwrap_err();
        assert!(matches!(err, StunError::TransactionMismatch));
    }

    #[test]
    fn test_parse_binding_response_no_mapped_address() {
        let txn_id = [0u8; 12];
        let mut data = [0u8; 20];
        data[0..2].copy_from_slice(&BINDING_RESPONSE.to_be_bytes());
        // attr_len = 0
        data[4..8].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
        data[8..20].copy_from_slice(&txn_id);

        let err = parse_binding_response(&data, &txn_id).unwrap_err();
        assert!(matches!(err, StunError::NoMappedAddress));
    }

    #[test]
    fn test_parse_binding_response_mapped_address_fallback() {
        // Test that MAPPED-ADDRESS (0x0001) is used when XOR-MAPPED-ADDRESS
        // is absent (legacy STUN server).
        let txn_id = [0u8; 12];
        let ip = Ipv4Addr::new(203, 0, 113, 5);
        let port: u16 = 40000;

        // Build MAPPED-ADDRESS attribute value
        let mut attr_value = [0u8; 8];
        attr_value[0] = 0x00;
        attr_value[1] = 0x01; // IPv4
        attr_value[2..4].copy_from_slice(&port.to_be_bytes());
        attr_value[4..8].copy_from_slice(&ip.octets());

        let attr_header_len: u16 = 4 + 8;
        let mut response = Vec::with_capacity(STUN_HEADER_SIZE + attr_header_len as usize);

        response.extend_from_slice(&BINDING_RESPONSE.to_be_bytes());
        response.extend_from_slice(&attr_header_len.to_be_bytes());
        response.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
        response.extend_from_slice(&txn_id);

        response.extend_from_slice(&ATTR_MAPPED_ADDRESS.to_be_bytes());
        response.extend_from_slice(&8u16.to_be_bytes());
        response.extend_from_slice(&attr_value);

        let result = parse_binding_response(&response, &txn_id).unwrap();
        assert_eq!(result, SocketAddr::new(ip.into(), port));
    }

    #[test]
    fn test_symmetric_nat_detection_same_ports() {
        let results = vec![
            ReflexiveAddress {
                address: SocketAddr::new(Ipv4Addr::new(1, 2, 3, 4).into(), 5000),
                server: "server1".to_string(),
            },
            ReflexiveAddress {
                address: SocketAddr::new(Ipv4Addr::new(5, 6, 7, 8).into(), 5000),
                server: "server2".to_string(),
            },
        ];
        assert_eq!(
            detect_nat_behavior(&results),
            NatBehavior::EndpointIndependent
        );
    }

    #[test]
    fn test_symmetric_nat_detection_different_ports() {
        let results = vec![
            ReflexiveAddress {
                address: SocketAddr::new(Ipv4Addr::new(1, 2, 3, 4).into(), 5000),
                server: "server1".to_string(),
            },
            ReflexiveAddress {
                address: SocketAddr::new(Ipv4Addr::new(1, 2, 3, 4).into(), 6000),
                server: "server2".to_string(),
            },
        ];
        assert_eq!(detect_nat_behavior(&results), NatBehavior::Symmetric);
    }

    #[test]
    fn test_symmetric_nat_detection_single_result() {
        let results = vec![ReflexiveAddress {
            address: SocketAddr::new(Ipv4Addr::new(1, 2, 3, 4).into(), 5000),
            server: "server1".to_string(),
        }];
        assert_eq!(
            detect_nat_behavior(&results),
            NatBehavior::EndpointIndependent
        );
    }

    #[test]
    fn test_symmetric_nat_detection_empty() {
        let results: Vec<ReflexiveAddress> = vec![];
        assert_eq!(
            detect_nat_behavior(&results),
            NatBehavior::EndpointIndependent
        );
    }

    #[test]
    fn test_stun_client_new() {
        let servers = vec![StunServerConfig {
            address: "stun.example.com:3478".to_string(),
            label: Some("Test".to_string()),
        }];
        let client = StunClient::new(servers);
        assert_eq!(client.servers.len(), 1);
    }

    // ---- IPv6 tests ---------------------------------------------------------

    #[test]
    fn test_parse_xor_mapped_address_ipv6() {
        // Test reflexive address: [2001:db8::1]:54321
        let ip = Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 1);
        let port: u16 = 54321;
        let txn_id: [u8; 12] = [
            0xA1, 0xB2, 0xC3, 0xD4, 0xE5, 0xF6, 0x17, 0x28, 0x39, 0x4A, 0x5B, 0x6C,
        ];

        // XOR the port with high 16 bits of magic cookie
        let xored_port = port ^ (MAGIC_COOKIE >> 16) as u16;

        // XOR the IP: first 4 bytes with magic cookie, remaining 12 with txn_id
        let ip_bytes = ip.octets();
        let cookie_bytes = MAGIC_COOKIE.to_be_bytes();
        let mut xored_ip = [0u8; 16];
        for i in 0..16 {
            if i < 4 {
                xored_ip[i] = ip_bytes[i] ^ cookie_bytes[i];
            } else {
                xored_ip[i] = ip_bytes[i] ^ txn_id[i - 4];
            }
        }

        // Build attribute value: [reserved(1), family(1), port(2), ip(16)] = 20 bytes
        let mut value = [0u8; 20];
        value[0] = 0x00; // reserved
        value[1] = 0x02; // IPv6
        value[2..4].copy_from_slice(&xored_port.to_be_bytes());
        value[4..20].copy_from_slice(&xored_ip);

        let result = parse_xor_mapped_address(&value, &txn_id).unwrap();
        assert_eq!(result.ip(), ip);
        assert_eq!(result.port(), port);
    }

    #[test]
    fn test_parse_xor_mapped_address_ipv6_all_ones() {
        // Edge case: all-ones IPv6 address
        let ip = Ipv6Addr::new(
            0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF,
        );
        let port: u16 = 65535;
        let txn_id = [0xFF; 12];

        let xored_port = port ^ (MAGIC_COOKIE >> 16) as u16;
        let ip_bytes = ip.octets();
        let cookie_bytes = MAGIC_COOKIE.to_be_bytes();
        let mut xored_ip = [0u8; 16];
        for i in 0..16 {
            if i < 4 {
                xored_ip[i] = ip_bytes[i] ^ cookie_bytes[i];
            } else {
                xored_ip[i] = ip_bytes[i] ^ txn_id[i - 4];
            }
        }

        let mut value = [0u8; 20];
        value[0] = 0x00;
        value[1] = 0x02;
        value[2..4].copy_from_slice(&xored_port.to_be_bytes());
        value[4..20].copy_from_slice(&xored_ip);

        let result = parse_xor_mapped_address(&value, &txn_id).unwrap();
        assert_eq!(result.ip(), ip);
        assert_eq!(result.port(), port);
    }

    #[test]
    fn test_parse_mapped_address_ipv6() {
        let ip = Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 1);
        let port: u16 = 12345;

        let mut value = [0u8; 20];
        value[0] = 0x00; // reserved
        value[1] = 0x02; // IPv6
        value[2..4].copy_from_slice(&port.to_be_bytes());
        value[4..20].copy_from_slice(&ip.octets());

        let result = parse_mapped_address(&value).unwrap();
        assert_eq!(result.ip(), ip);
        assert_eq!(result.port(), port);
    }

    #[test]
    fn test_parse_binding_response_xor_mapped_ipv6() {
        // Full binding response with an IPv6 XOR-MAPPED-ADDRESS
        let ip = Ipv6Addr::new(0x2001, 0x0db8, 0xABCD, 0, 0, 0, 0, 0x42);
        let port: u16 = 9876;
        let txn_id: [u8; 12] = [
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC,
        ];

        let xored_port = port ^ (MAGIC_COOKIE >> 16) as u16;
        let ip_bytes = ip.octets();
        let cookie_bytes = MAGIC_COOKIE.to_be_bytes();
        let mut xored_ip = [0u8; 16];
        for i in 0..16 {
            if i < 4 {
                xored_ip[i] = ip_bytes[i] ^ cookie_bytes[i];
            } else {
                xored_ip[i] = ip_bytes[i] ^ txn_id[i - 4];
            }
        }

        // Build XOR-MAPPED-ADDRESS attribute value (20 bytes for IPv6)
        let mut attr_value = [0u8; 20];
        attr_value[0] = 0x00; // reserved
        attr_value[1] = 0x02; // IPv6
        attr_value[2..4].copy_from_slice(&xored_port.to_be_bytes());
        attr_value[4..20].copy_from_slice(&xored_ip);

        // Attribute: type(2) + length(2) + value(20) = 24 bytes
        let attr_header_len: u16 = 4 + 20;
        let mut response = Vec::with_capacity(STUN_HEADER_SIZE + attr_header_len as usize);

        // Header
        response.extend_from_slice(&BINDING_RESPONSE.to_be_bytes());
        response.extend_from_slice(&attr_header_len.to_be_bytes());
        response.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
        response.extend_from_slice(&txn_id);

        // XOR-MAPPED-ADDRESS attribute
        response.extend_from_slice(&ATTR_XOR_MAPPED_ADDRESS.to_be_bytes());
        response.extend_from_slice(&20u16.to_be_bytes()); // value length = 20 for IPv6
        response.extend_from_slice(&attr_value);

        let result = parse_binding_response(&response, &txn_id).unwrap();
        assert_eq!(result, SocketAddr::new(ip.into(), port));
    }

    #[test]
    fn test_parse_binding_response_mapped_address_ipv6_fallback() {
        // Test that MAPPED-ADDRESS (0x0001) with IPv6 is used as fallback
        let ip = Ipv6Addr::new(0xFE80, 0, 0, 0, 0, 0, 0, 1);
        let port: u16 = 40000;
        let txn_id = [0u8; 12];

        let mut attr_value = [0u8; 20];
        attr_value[0] = 0x00;
        attr_value[1] = 0x02; // IPv6
        attr_value[2..4].copy_from_slice(&port.to_be_bytes());
        attr_value[4..20].copy_from_slice(&ip.octets());

        let attr_header_len: u16 = 4 + 20;
        let mut response = Vec::with_capacity(STUN_HEADER_SIZE + attr_header_len as usize);

        response.extend_from_slice(&BINDING_RESPONSE.to_be_bytes());
        response.extend_from_slice(&attr_header_len.to_be_bytes());
        response.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
        response.extend_from_slice(&txn_id);

        response.extend_from_slice(&ATTR_MAPPED_ADDRESS.to_be_bytes());
        response.extend_from_slice(&20u16.to_be_bytes());
        response.extend_from_slice(&attr_value);

        let result = parse_binding_response(&response, &txn_id).unwrap();
        assert_eq!(result, SocketAddr::new(ip.into(), port));
    }

    #[test]
    fn test_symmetric_nat_detection_ipv6_same_ports() {
        let results = vec![
            ReflexiveAddress {
                address: SocketAddr::new(
                    Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1).into(),
                    5000,
                ),
                server: "server1".to_string(),
            },
            ReflexiveAddress {
                address: SocketAddr::new(
                    Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 2).into(),
                    5000,
                ),
                server: "server2".to_string(),
            },
        ];
        assert_eq!(
            detect_nat_behavior(&results),
            NatBehavior::EndpointIndependent
        );
    }

    #[test]
    fn test_symmetric_nat_detection_ipv6_different_ports() {
        let results = vec![
            ReflexiveAddress {
                address: SocketAddr::new(
                    Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1).into(),
                    5000,
                ),
                server: "server1".to_string(),
            },
            ReflexiveAddress {
                address: SocketAddr::new(
                    Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1).into(),
                    6000,
                ),
                server: "server2".to_string(),
            },
        ];
        assert_eq!(detect_nat_behavior(&results), NatBehavior::Symmetric);
    }

    #[test]
    fn test_parse_xor_mapped_address_ipv6_too_short() {
        // IPv6 attribute value needs 20 bytes; provide only 18
        let txn_id = [0u8; 12];
        let mut value = [0u8; 18];
        value[1] = 0x02; // IPv6 family
        assert!(parse_xor_mapped_address(&value, &txn_id).is_none());
    }

    #[test]
    fn test_parse_mapped_address_ipv6_too_short() {
        // IPv6 mapped address needs 20 bytes; provide only 10
        let mut value = [0u8; 10];
        value[1] = 0x02; // IPv6 family
        assert!(parse_mapped_address(&value).is_none());
    }
}
