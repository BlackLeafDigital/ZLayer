//! Binary message protocol for tunnel communication
//!
//! Message format:
//! ```text
//! +----------+----------+----------------------------------+
//! | Type(1)  | Len(4)   | Payload (variable)               |
//! +----------+----------+----------------------------------+
//! ```

use crate::error::{Result, TunnelError};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Protocol version
pub const PROTOCOL_VERSION: u8 = 1;

/// Maximum message size (64KB)
pub const MAX_MESSAGE_SIZE: usize = 65536;

/// Header size (1 byte type + 4 bytes length)
pub const HEADER_SIZE: usize = 5;

/// Message type discriminants
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    /// Client authentication request
    Auth = 0x01,
    /// Server authentication success
    AuthOk = 0x02,
    /// Server authentication failure
    AuthFail = 0x03,
    /// Client service registration
    Register = 0x10,
    /// Server registration success
    RegisterOk = 0x11,
    /// Server registration failure
    RegisterFail = 0x12,
    /// Server connection announcement
    Connect = 0x20,
    /// Client connection acknowledgment
    ConnectAck = 0x21,
    /// Client connection failure
    ConnectFail = 0x22,
    /// Heartbeat (bidirectional)
    Heartbeat = 0x30,
    /// Heartbeat acknowledgment (bidirectional)
    HeartbeatAck = 0x31,
    /// Client service unregistration
    Unregister = 0x40,
    /// Server disconnect notification
    Disconnect = 0x41,
}

impl TryFrom<u8> for MessageType {
    type Error = TunnelError;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            0x01 => Ok(Self::Auth),
            0x02 => Ok(Self::AuthOk),
            0x03 => Ok(Self::AuthFail),
            0x10 => Ok(Self::Register),
            0x11 => Ok(Self::RegisterOk),
            0x12 => Ok(Self::RegisterFail),
            0x20 => Ok(Self::Connect),
            0x21 => Ok(Self::ConnectAck),
            0x22 => Ok(Self::ConnectFail),
            0x30 => Ok(Self::Heartbeat),
            0x31 => Ok(Self::HeartbeatAck),
            0x40 => Ok(Self::Unregister),
            0x41 => Ok(Self::Disconnect),
            _ => Err(TunnelError::protocol(format!(
                "unknown message type: 0x{value:02x}"
            ))),
        }
    }
}

/// Service protocol type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ServiceProtocol {
    /// TCP protocol
    #[default]
    Tcp,
    /// UDP protocol
    Udp,
}

impl ServiceProtocol {
    /// Convert to wire format byte
    #[must_use]
    pub const fn to_byte(self) -> u8 {
        match self {
            Self::Tcp => 0,
            Self::Udp => 1,
        }
    }

    /// Parse from wire format byte
    ///
    /// # Errors
    ///
    /// Returns an error if the byte is not a valid protocol type (0 or 1).
    pub fn from_byte(byte: u8) -> Result<Self> {
        match byte {
            0 => Ok(Self::Tcp),
            1 => Ok(Self::Udp),
            _ => Err(TunnelError::protocol(format!(
                "unknown protocol type: {byte}"
            ))),
        }
    }
}

/// Protocol messages
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// Client authentication request (C->S)
    Auth {
        /// Authentication token
        token: String,
        /// Client identifier
        client_id: Uuid,
    },

    /// Server authentication success (S->C)
    AuthOk {
        /// Assigned tunnel ID
        tunnel_id: Uuid,
    },

    /// Server authentication failure (S->C)
    AuthFail {
        /// Failure reason
        reason: String,
    },

    /// Client service registration (C->S)
    Register {
        /// Service name
        name: String,
        /// Protocol type
        protocol: ServiceProtocol,
        /// Local port on client
        local_port: u16,
        /// Requested remote port (0 = auto-assign)
        remote_port: u16,
    },

    /// Server registration success (S->C)
    RegisterOk {
        /// Assigned service ID
        service_id: Uuid,
    },

    /// Server registration failure (S->C)
    RegisterFail {
        /// Failure reason
        reason: String,
    },

    /// Server connection announcement (S->C)
    Connect {
        /// Service ID for this connection
        service_id: Uuid,
        /// Unique connection ID
        connection_id: Uuid,
        /// Client address (IP:port)
        client_addr: String,
    },

    /// Client connection acknowledgment (C->S)
    ConnectAck {
        /// Connection ID being acknowledged
        connection_id: Uuid,
    },

    /// Client connection failure (C->S)
    ConnectFail {
        /// Connection ID that failed
        connection_id: Uuid,
        /// Failure reason
        reason: String,
    },

    /// Heartbeat (bidirectional)
    Heartbeat {
        /// Unix timestamp in milliseconds
        timestamp: u64,
    },

    /// Heartbeat acknowledgment (bidirectional)
    HeartbeatAck {
        /// Echo of original timestamp
        timestamp: u64,
    },

    /// Client service unregistration (C->S)
    Unregister {
        /// Service ID to unregister
        service_id: Uuid,
    },

    /// Server disconnect notification (S->C)
    Disconnect {
        /// Disconnect reason
        reason: String,
    },
}

impl Message {
    /// Get the message type
    #[must_use]
    pub const fn message_type(&self) -> MessageType {
        match self {
            Self::Auth { .. } => MessageType::Auth,
            Self::AuthOk { .. } => MessageType::AuthOk,
            Self::AuthFail { .. } => MessageType::AuthFail,
            Self::Register { .. } => MessageType::Register,
            Self::RegisterOk { .. } => MessageType::RegisterOk,
            Self::RegisterFail { .. } => MessageType::RegisterFail,
            Self::Connect { .. } => MessageType::Connect,
            Self::ConnectAck { .. } => MessageType::ConnectAck,
            Self::ConnectFail { .. } => MessageType::ConnectFail,
            Self::Heartbeat { .. } => MessageType::Heartbeat,
            Self::HeartbeatAck { .. } => MessageType::HeartbeatAck,
            Self::Unregister { .. } => MessageType::Unregister,
            Self::Disconnect { .. } => MessageType::Disconnect,
        }
    }

    /// Encode the message to binary format
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::match_same_arms)]
    pub fn encode(&self) -> Vec<u8> {
        let mut payload = Vec::new();

        match self {
            Self::Auth { token, client_id } => {
                // token_len(2) + token + client_id(16)
                let token_bytes = token.as_bytes();
                payload.extend_from_slice(&(token_bytes.len() as u16).to_be_bytes());
                payload.extend_from_slice(token_bytes);
                payload.extend_from_slice(client_id.as_bytes());
            }

            Self::AuthOk { tunnel_id } => {
                // tunnel_id(16)
                payload.extend_from_slice(tunnel_id.as_bytes());
            }

            Self::AuthFail { reason } => {
                // reason_len(2) + reason
                let reason_bytes = reason.as_bytes();
                payload.extend_from_slice(&(reason_bytes.len() as u16).to_be_bytes());
                payload.extend_from_slice(reason_bytes);
            }

            Self::Register {
                name,
                protocol,
                local_port,
                remote_port,
            } => {
                // name_len(1) + name + protocol(1) + local_port(2) + remote_port(2)
                let name_bytes = name.as_bytes();
                payload.push(name_bytes.len() as u8);
                payload.extend_from_slice(name_bytes);
                payload.push(protocol.to_byte());
                payload.extend_from_slice(&local_port.to_be_bytes());
                payload.extend_from_slice(&remote_port.to_be_bytes());
            }

            Self::RegisterOk { service_id } => {
                // service_id(16)
                payload.extend_from_slice(service_id.as_bytes());
            }

            Self::RegisterFail { reason } => {
                // reason_len(2) + reason
                let reason_bytes = reason.as_bytes();
                payload.extend_from_slice(&(reason_bytes.len() as u16).to_be_bytes());
                payload.extend_from_slice(reason_bytes);
            }

            Self::Connect {
                service_id,
                connection_id,
                client_addr,
            } => {
                // service_id(16) + connection_id(16) + addr_len(2) + client_addr
                payload.extend_from_slice(service_id.as_bytes());
                payload.extend_from_slice(connection_id.as_bytes());
                let addr_bytes = client_addr.as_bytes();
                payload.extend_from_slice(&(addr_bytes.len() as u16).to_be_bytes());
                payload.extend_from_slice(addr_bytes);
            }

            Self::ConnectAck { connection_id } => {
                // connection_id(16)
                payload.extend_from_slice(connection_id.as_bytes());
            }

            Self::ConnectFail {
                connection_id,
                reason,
            } => {
                // connection_id(16) + reason_len(2) + reason
                payload.extend_from_slice(connection_id.as_bytes());
                let reason_bytes = reason.as_bytes();
                payload.extend_from_slice(&(reason_bytes.len() as u16).to_be_bytes());
                payload.extend_from_slice(reason_bytes);
            }

            Self::Heartbeat { timestamp } | Self::HeartbeatAck { timestamp } => {
                // timestamp(8)
                payload.extend_from_slice(&timestamp.to_be_bytes());
            }

            Self::Unregister { service_id } => {
                // service_id(16)
                payload.extend_from_slice(service_id.as_bytes());
            }

            Self::Disconnect { reason } => {
                // reason_len(2) + reason
                let reason_bytes = reason.as_bytes();
                payload.extend_from_slice(&(reason_bytes.len() as u16).to_be_bytes());
                payload.extend_from_slice(reason_bytes);
            }
        }

        // Build final message: type(1) + len(4) + payload
        let msg_type = self.message_type() as u8;
        let payload_len = payload.len() as u32;

        let mut result = Vec::with_capacity(HEADER_SIZE + payload.len());
        result.push(msg_type);
        result.extend_from_slice(&payload_len.to_be_bytes());
        result.extend_from_slice(&payload);

        result
    }

    /// Decode a message from binary format
    ///
    /// Returns the decoded message and the number of bytes consumed.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The buffer is too short for a complete message
    /// - The message type is unknown
    /// - The payload is malformed or contains invalid data
    pub fn decode(bytes: &[u8]) -> Result<(Self, usize)> {
        if bytes.len() < HEADER_SIZE {
            return Err(TunnelError::protocol(format!(
                "message too short: {} bytes, need at least {}",
                bytes.len(),
                HEADER_SIZE
            )));
        }

        let msg_type = MessageType::try_from(bytes[0])?;
        let payload_len = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]) as usize;

        if payload_len > MAX_MESSAGE_SIZE - HEADER_SIZE {
            return Err(TunnelError::protocol(format!(
                "payload too large: {payload_len} bytes, max {}",
                MAX_MESSAGE_SIZE - HEADER_SIZE
            )));
        }

        let total_len = HEADER_SIZE + payload_len;
        if bytes.len() < total_len {
            return Err(TunnelError::protocol(format!(
                "incomplete message: have {} bytes, need {}",
                bytes.len(),
                total_len
            )));
        }

        let payload = &bytes[HEADER_SIZE..total_len];
        let message = Self::decode_payload(msg_type, payload)?;

        Ok((message, total_len))
    }

    /// Decode the payload for a given message type
    #[allow(clippy::too_many_lines)]
    fn decode_payload(msg_type: MessageType, payload: &[u8]) -> Result<Self> {
        match msg_type {
            MessageType::Auth => Self::decode_auth(payload),
            MessageType::AuthOk => Self::decode_auth_ok(payload),
            MessageType::AuthFail => Self::decode_auth_fail(payload),
            MessageType::Register => Self::decode_register(payload),
            MessageType::RegisterOk => Self::decode_register_ok(payload),
            MessageType::RegisterFail => Self::decode_register_fail(payload),
            MessageType::Connect => Self::decode_connect(payload),
            MessageType::ConnectAck => Self::decode_connect_ack(payload),
            MessageType::ConnectFail => Self::decode_connect_fail(payload),
            MessageType::Heartbeat => Self::decode_heartbeat(payload),
            MessageType::HeartbeatAck => Self::decode_heartbeat_ack(payload),
            MessageType::Unregister => Self::decode_unregister(payload),
            MessageType::Disconnect => Self::decode_disconnect(payload),
        }
    }

    fn decode_auth(payload: &[u8]) -> Result<Self> {
        // token_len(2) + token + client_id(16)
        if payload.len() < 2 {
            return Err(TunnelError::protocol(
                "Auth: payload too short for token length",
            ));
        }
        let token_len = u16::from_be_bytes([payload[0], payload[1]]) as usize;
        if payload.len() < 2 + token_len + 16 {
            return Err(TunnelError::protocol("Auth: payload too short"));
        }
        let token = String::from_utf8(payload[2..2 + token_len].to_vec())
            .map_err(|e| TunnelError::protocol(format!("Auth: invalid token UTF-8: {e}")))?;
        let client_id = Uuid::from_slice(&payload[2 + token_len..2 + token_len + 16])
            .map_err(|e| TunnelError::protocol(format!("Auth: invalid client_id: {e}")))?;
        Ok(Self::Auth { token, client_id })
    }

    fn decode_auth_ok(payload: &[u8]) -> Result<Self> {
        // tunnel_id(16)
        if payload.len() < 16 {
            return Err(TunnelError::protocol("AuthOk: payload too short"));
        }
        let tunnel_id = Uuid::from_slice(&payload[..16])
            .map_err(|e| TunnelError::protocol(format!("AuthOk: invalid tunnel_id: {e}")))?;
        Ok(Self::AuthOk { tunnel_id })
    }

    fn decode_auth_fail(payload: &[u8]) -> Result<Self> {
        // reason_len(2) + reason
        if payload.len() < 2 {
            return Err(TunnelError::protocol(
                "AuthFail: payload too short for reason length",
            ));
        }
        let reason_len = u16::from_be_bytes([payload[0], payload[1]]) as usize;
        if payload.len() < 2 + reason_len {
            return Err(TunnelError::protocol(
                "AuthFail: payload too short for reason",
            ));
        }
        let reason = String::from_utf8(payload[2..2 + reason_len].to_vec())
            .map_err(|e| TunnelError::protocol(format!("AuthFail: invalid reason UTF-8: {e}")))?;
        Ok(Self::AuthFail { reason })
    }

    fn decode_register(payload: &[u8]) -> Result<Self> {
        // name_len(1) + name + protocol(1) + local_port(2) + remote_port(2)
        if payload.is_empty() {
            return Err(TunnelError::protocol(
                "Register: payload too short for name length",
            ));
        }
        let name_len = payload[0] as usize;
        if payload.len() < 1 + name_len + 1 + 2 + 2 {
            return Err(TunnelError::protocol("Register: payload too short"));
        }
        let name = String::from_utf8(payload[1..=name_len].to_vec())
            .map_err(|e| TunnelError::protocol(format!("Register: invalid name UTF-8: {e}")))?;
        let protocol = ServiceProtocol::from_byte(payload[1 + name_len])?;
        let local_port = u16::from_be_bytes([payload[2 + name_len], payload[3 + name_len]]);
        let remote_port = u16::from_be_bytes([payload[4 + name_len], payload[5 + name_len]]);
        Ok(Self::Register {
            name,
            protocol,
            local_port,
            remote_port,
        })
    }

    fn decode_register_ok(payload: &[u8]) -> Result<Self> {
        // service_id(16)
        if payload.len() < 16 {
            return Err(TunnelError::protocol("RegisterOk: payload too short"));
        }
        let service_id = Uuid::from_slice(&payload[..16])
            .map_err(|e| TunnelError::protocol(format!("RegisterOk: invalid service_id: {e}")))?;
        Ok(Self::RegisterOk { service_id })
    }

    fn decode_register_fail(payload: &[u8]) -> Result<Self> {
        // reason_len(2) + reason
        if payload.len() < 2 {
            return Err(TunnelError::protocol(
                "RegisterFail: payload too short for reason length",
            ));
        }
        let reason_len = u16::from_be_bytes([payload[0], payload[1]]) as usize;
        if payload.len() < 2 + reason_len {
            return Err(TunnelError::protocol(
                "RegisterFail: payload too short for reason",
            ));
        }
        let reason = String::from_utf8(payload[2..2 + reason_len].to_vec()).map_err(|e| {
            TunnelError::protocol(format!("RegisterFail: invalid reason UTF-8: {e}"))
        })?;
        Ok(Self::RegisterFail { reason })
    }

    fn decode_connect(payload: &[u8]) -> Result<Self> {
        // service_id(16) + connection_id(16) + addr_len(2) + client_addr
        if payload.len() < 16 + 16 + 2 {
            return Err(TunnelError::protocol("Connect: payload too short"));
        }
        let service_id = Uuid::from_slice(&payload[..16])
            .map_err(|e| TunnelError::protocol(format!("Connect: invalid service_id: {e}")))?;
        let connection_id = Uuid::from_slice(&payload[16..32])
            .map_err(|e| TunnelError::protocol(format!("Connect: invalid connection_id: {e}")))?;
        let addr_len = u16::from_be_bytes([payload[32], payload[33]]) as usize;
        if payload.len() < 34 + addr_len {
            return Err(TunnelError::protocol(
                "Connect: payload too short for client_addr",
            ));
        }
        let client_addr = String::from_utf8(payload[34..34 + addr_len].to_vec()).map_err(|e| {
            TunnelError::protocol(format!("Connect: invalid client_addr UTF-8: {e}"))
        })?;
        Ok(Self::Connect {
            service_id,
            connection_id,
            client_addr,
        })
    }

    fn decode_connect_ack(payload: &[u8]) -> Result<Self> {
        // connection_id(16)
        if payload.len() < 16 {
            return Err(TunnelError::protocol("ConnectAck: payload too short"));
        }
        let connection_id = Uuid::from_slice(&payload[..16]).map_err(|e| {
            TunnelError::protocol(format!("ConnectAck: invalid connection_id: {e}"))
        })?;
        Ok(Self::ConnectAck { connection_id })
    }

    fn decode_connect_fail(payload: &[u8]) -> Result<Self> {
        // connection_id(16) + reason_len(2) + reason
        if payload.len() < 16 + 2 {
            return Err(TunnelError::protocol("ConnectFail: payload too short"));
        }
        let connection_id = Uuid::from_slice(&payload[..16]).map_err(|e| {
            TunnelError::protocol(format!("ConnectFail: invalid connection_id: {e}"))
        })?;
        let reason_len = u16::from_be_bytes([payload[16], payload[17]]) as usize;
        if payload.len() < 18 + reason_len {
            return Err(TunnelError::protocol(
                "ConnectFail: payload too short for reason",
            ));
        }
        let reason = String::from_utf8(payload[18..18 + reason_len].to_vec()).map_err(|e| {
            TunnelError::protocol(format!("ConnectFail: invalid reason UTF-8: {e}"))
        })?;
        Ok(Self::ConnectFail {
            connection_id,
            reason,
        })
    }

    fn decode_heartbeat(payload: &[u8]) -> Result<Self> {
        // timestamp(8)
        if payload.len() < 8 {
            return Err(TunnelError::protocol("Heartbeat: payload too short"));
        }
        let timestamp = u64::from_be_bytes([
            payload[0], payload[1], payload[2], payload[3], payload[4], payload[5], payload[6],
            payload[7],
        ]);
        Ok(Self::Heartbeat { timestamp })
    }

    fn decode_heartbeat_ack(payload: &[u8]) -> Result<Self> {
        // timestamp(8)
        if payload.len() < 8 {
            return Err(TunnelError::protocol("HeartbeatAck: payload too short"));
        }
        let timestamp = u64::from_be_bytes([
            payload[0], payload[1], payload[2], payload[3], payload[4], payload[5], payload[6],
            payload[7],
        ]);
        Ok(Self::HeartbeatAck { timestamp })
    }

    fn decode_unregister(payload: &[u8]) -> Result<Self> {
        // service_id(16)
        if payload.len() < 16 {
            return Err(TunnelError::protocol("Unregister: payload too short"));
        }
        let service_id = Uuid::from_slice(&payload[..16])
            .map_err(|e| TunnelError::protocol(format!("Unregister: invalid service_id: {e}")))?;
        Ok(Self::Unregister { service_id })
    }

    fn decode_disconnect(payload: &[u8]) -> Result<Self> {
        // reason_len(2) + reason
        if payload.len() < 2 {
            return Err(TunnelError::protocol(
                "Disconnect: payload too short for reason length",
            ));
        }
        let reason_len = u16::from_be_bytes([payload[0], payload[1]]) as usize;
        if payload.len() < 2 + reason_len {
            return Err(TunnelError::protocol(
                "Disconnect: payload too short for reason",
            ));
        }
        let reason = String::from_utf8(payload[2..2 + reason_len].to_vec())
            .map_err(|e| TunnelError::protocol(format!("Disconnect: invalid reason UTF-8: {e}")))?;
        Ok(Self::Disconnect { reason })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to test encode/decode roundtrip
    fn roundtrip(msg: Message) {
        let encoded = msg.encode();
        let (decoded, consumed) = Message::decode(&encoded).expect("decode failed");
        assert_eq!(consumed, encoded.len(), "consumed bytes mismatch");
        assert_eq!(decoded, msg, "roundtrip mismatch");
    }

    #[test]
    fn test_auth_roundtrip() {
        roundtrip(Message::Auth {
            token: "tun_abc123".to_string(),
            client_id: Uuid::new_v4(),
        });

        // Test with empty token
        roundtrip(Message::Auth {
            token: String::new(),
            client_id: Uuid::nil(),
        });

        // Test with long token
        roundtrip(Message::Auth {
            token: "a".repeat(1000),
            client_id: Uuid::new_v4(),
        });
    }

    #[test]
    fn test_auth_ok_roundtrip() {
        roundtrip(Message::AuthOk {
            tunnel_id: Uuid::new_v4(),
        });

        roundtrip(Message::AuthOk {
            tunnel_id: Uuid::nil(),
        });
    }

    #[test]
    fn test_auth_fail_roundtrip() {
        roundtrip(Message::AuthFail {
            reason: "invalid token".to_string(),
        });

        roundtrip(Message::AuthFail {
            reason: String::new(),
        });

        roundtrip(Message::AuthFail {
            reason: "x".repeat(500),
        });
    }

    #[test]
    fn test_register_roundtrip() {
        roundtrip(Message::Register {
            name: "ssh".to_string(),
            protocol: ServiceProtocol::Tcp,
            local_port: 22,
            remote_port: 2222,
        });

        roundtrip(Message::Register {
            name: "game".to_string(),
            protocol: ServiceProtocol::Udp,
            local_port: 27015,
            remote_port: 0, // Auto-assign
        });

        roundtrip(Message::Register {
            name: "a".repeat(255), // Max name length with u8
            protocol: ServiceProtocol::Tcp,
            local_port: 65535,
            remote_port: 65535,
        });
    }

    #[test]
    fn test_register_ok_roundtrip() {
        roundtrip(Message::RegisterOk {
            service_id: Uuid::new_v4(),
        });
    }

    #[test]
    fn test_register_fail_roundtrip() {
        roundtrip(Message::RegisterFail {
            reason: "port already in use".to_string(),
        });
    }

    #[test]
    fn test_connect_roundtrip() {
        roundtrip(Message::Connect {
            service_id: Uuid::new_v4(),
            connection_id: Uuid::new_v4(),
            client_addr: "192.168.1.100:54321".to_string(),
        });

        roundtrip(Message::Connect {
            service_id: Uuid::new_v4(),
            connection_id: Uuid::new_v4(),
            client_addr: "[::1]:8080".to_string(),
        });
    }

    #[test]
    fn test_connect_ack_roundtrip() {
        roundtrip(Message::ConnectAck {
            connection_id: Uuid::new_v4(),
        });
    }

    #[test]
    fn test_connect_fail_roundtrip() {
        roundtrip(Message::ConnectFail {
            connection_id: Uuid::new_v4(),
            reason: "connection refused".to_string(),
        });
    }

    #[test]
    fn test_heartbeat_roundtrip() {
        roundtrip(Message::Heartbeat {
            timestamp: 1_705_320_000_000,
        });

        roundtrip(Message::Heartbeat { timestamp: 0 });

        roundtrip(Message::Heartbeat {
            timestamp: u64::MAX,
        });
    }

    #[test]
    fn test_heartbeat_ack_roundtrip() {
        roundtrip(Message::HeartbeatAck {
            timestamp: 1_705_320_000_000,
        });
    }

    #[test]
    fn test_unregister_roundtrip() {
        roundtrip(Message::Unregister {
            service_id: Uuid::new_v4(),
        });
    }

    #[test]
    fn test_disconnect_roundtrip() {
        roundtrip(Message::Disconnect {
            reason: "server shutdown".to_string(),
        });
    }

    #[test]
    fn test_message_type_discriminants() {
        assert_eq!(
            Message::Auth {
                token: String::new(),
                client_id: Uuid::nil()
            }
            .message_type(),
            MessageType::Auth
        );
        assert_eq!(
            Message::AuthOk {
                tunnel_id: Uuid::nil()
            }
            .message_type(),
            MessageType::AuthOk
        );
        assert_eq!(
            Message::AuthFail {
                reason: String::new()
            }
            .message_type(),
            MessageType::AuthFail
        );
        assert_eq!(
            Message::Register {
                name: String::new(),
                protocol: ServiceProtocol::Tcp,
                local_port: 0,
                remote_port: 0
            }
            .message_type(),
            MessageType::Register
        );
        assert_eq!(
            Message::RegisterOk {
                service_id: Uuid::nil()
            }
            .message_type(),
            MessageType::RegisterOk
        );
        assert_eq!(
            Message::RegisterFail {
                reason: String::new()
            }
            .message_type(),
            MessageType::RegisterFail
        );
        assert_eq!(
            Message::Connect {
                service_id: Uuid::nil(),
                connection_id: Uuid::nil(),
                client_addr: String::new()
            }
            .message_type(),
            MessageType::Connect
        );
        assert_eq!(
            Message::ConnectAck {
                connection_id: Uuid::nil()
            }
            .message_type(),
            MessageType::ConnectAck
        );
        assert_eq!(
            Message::ConnectFail {
                connection_id: Uuid::nil(),
                reason: String::new()
            }
            .message_type(),
            MessageType::ConnectFail
        );
        assert_eq!(
            Message::Heartbeat { timestamp: 0 }.message_type(),
            MessageType::Heartbeat
        );
        assert_eq!(
            Message::HeartbeatAck { timestamp: 0 }.message_type(),
            MessageType::HeartbeatAck
        );
        assert_eq!(
            Message::Unregister {
                service_id: Uuid::nil()
            }
            .message_type(),
            MessageType::Unregister
        );
        assert_eq!(
            Message::Disconnect {
                reason: String::new()
            }
            .message_type(),
            MessageType::Disconnect
        );
    }

    #[test]
    fn test_message_type_from_u8() {
        assert_eq!(MessageType::try_from(0x01).unwrap(), MessageType::Auth);
        assert_eq!(MessageType::try_from(0x02).unwrap(), MessageType::AuthOk);
        assert_eq!(MessageType::try_from(0x03).unwrap(), MessageType::AuthFail);
        assert_eq!(MessageType::try_from(0x10).unwrap(), MessageType::Register);
        assert_eq!(
            MessageType::try_from(0x11).unwrap(),
            MessageType::RegisterOk
        );
        assert_eq!(
            MessageType::try_from(0x12).unwrap(),
            MessageType::RegisterFail
        );
        assert_eq!(MessageType::try_from(0x20).unwrap(), MessageType::Connect);
        assert_eq!(
            MessageType::try_from(0x21).unwrap(),
            MessageType::ConnectAck
        );
        assert_eq!(
            MessageType::try_from(0x22).unwrap(),
            MessageType::ConnectFail
        );
        assert_eq!(MessageType::try_from(0x30).unwrap(), MessageType::Heartbeat);
        assert_eq!(
            MessageType::try_from(0x31).unwrap(),
            MessageType::HeartbeatAck
        );
        assert_eq!(
            MessageType::try_from(0x40).unwrap(),
            MessageType::Unregister
        );
        assert_eq!(
            MessageType::try_from(0x41).unwrap(),
            MessageType::Disconnect
        );

        // Invalid type
        assert!(MessageType::try_from(0xFF).is_err());
        assert!(MessageType::try_from(0x00).is_err());
    }

    #[test]
    fn test_service_protocol_roundtrip() {
        assert_eq!(
            ServiceProtocol::from_byte(ServiceProtocol::Tcp.to_byte()).unwrap(),
            ServiceProtocol::Tcp
        );
        assert_eq!(
            ServiceProtocol::from_byte(ServiceProtocol::Udp.to_byte()).unwrap(),
            ServiceProtocol::Udp
        );
        assert!(ServiceProtocol::from_byte(0xFF).is_err());
    }

    #[test]
    fn test_decode_too_short() {
        // Less than header size
        assert!(Message::decode(&[]).is_err());
        assert!(Message::decode(&[0x01]).is_err());
        assert!(Message::decode(&[0x01, 0x00, 0x00, 0x00]).is_err());
    }

    #[test]
    fn test_decode_incomplete_payload() {
        // Valid header but incomplete payload
        let bytes = [0x01, 0x00, 0x00, 0x00, 0x20, 0x00]; // Says 32 bytes payload, but only 1 byte present
        assert!(Message::decode(&bytes).is_err());
    }

    #[test]
    fn test_decode_invalid_message_type() {
        let bytes = [0xFF, 0x00, 0x00, 0x00, 0x00]; // Invalid type, zero payload
        assert!(Message::decode(&bytes).is_err());
    }

    #[test]
    fn test_decode_payload_too_large() {
        // Payload size exceeds MAX_MESSAGE_SIZE
        let bytes = [0x01, 0xFF, 0xFF, 0xFF, 0xFF]; // ~4GB payload
        assert!(Message::decode(&bytes).is_err());
    }

    #[test]
    fn test_header_size_constant() {
        // Verify HEADER_SIZE is correct
        let msg = Message::Heartbeat { timestamp: 0 };
        let encoded = msg.encode();
        // Heartbeat has 8-byte payload
        assert_eq!(encoded.len(), HEADER_SIZE + 8);
    }

    #[test]
    fn test_multiple_messages_in_buffer() {
        let msg1 = Message::Heartbeat { timestamp: 100 };
        let msg2 = Message::HeartbeatAck { timestamp: 100 };

        let mut buffer = msg1.encode();
        buffer.extend_from_slice(&msg2.encode());

        // Decode first message
        let (decoded1, consumed1) = Message::decode(&buffer).unwrap();
        assert_eq!(decoded1, msg1);

        // Decode second message from remaining buffer
        let (decoded2, consumed2) = Message::decode(&buffer[consumed1..]).unwrap();
        assert_eq!(decoded2, msg2);

        assert_eq!(consumed1 + consumed2, buffer.len());
    }
}
