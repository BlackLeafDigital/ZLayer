//! WebSocket control channel handler for tunnel server
//!
//! This module implements the server-side control channel that handles
//! WebSocket connections from tunnel clients. It manages authentication,
//! service registration, heartbeat monitoring, and message routing.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use sha2::{Digest, Sha256};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{interval, timeout};
use tokio_tungstenite::{accept_async, tungstenite::Message as WsMessage, WebSocketStream};
use uuid::Uuid;

use crate::{
    ControlMessage, Message, Result, ServiceProtocol, TunnelError, TunnelRegistry,
    TunnelServerConfig,
};

/// Type alias for token validator function
pub type TokenValidator = Arc<dyn Fn(&str) -> Result<()> + Send + Sync>;

/// Get current timestamp in milliseconds since Unix epoch
///
/// Returns a u64 timestamp. In practice, timestamps won't overflow u64 for billions of years,
/// so we use saturating conversion for safety. The value is explicitly capped to `u64::MAX`
/// before casting, making truncation impossible.
#[inline]
#[allow(clippy::cast_possible_truncation)]
fn current_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
        .min(u128::from(u64::MAX)) as u64
}

/// Control channel handler for a single tunnel connection
///
/// The `ControlHandler` manages the lifecycle of a tunnel connection, including:
/// - WebSocket upgrade and authentication
/// - Service registration and unregistration
/// - Heartbeat monitoring
/// - Message routing between server and client
///
/// # Example
///
/// ```rust,no_run
/// use std::sync::Arc;
/// use zlayer_tunnel::{TunnelRegistry, TunnelServerConfig, ControlHandler, accept_all_tokens};
///
/// async fn handle_client(stream: tokio::net::TcpStream, addr: std::net::SocketAddr) {
///     let registry = Arc::new(TunnelRegistry::default());
///     let config = TunnelServerConfig::default();
///     let validator = Arc::new(accept_all_tokens);
///
///     let handler = ControlHandler::new(registry, config, validator);
///     if let Err(e) = handler.handle_connection(stream, addr).await {
///         tracing::error!("Connection error: {}", e);
///     }
/// }
/// ```
pub struct ControlHandler {
    registry: Arc<TunnelRegistry>,
    config: TunnelServerConfig,

    /// Token validator function (returns `Ok(())` if token is valid, `Err` with reason if not)
    token_validator: TokenValidator,
}

impl ControlHandler {
    /// Create a new control handler
    ///
    /// # Arguments
    ///
    /// * `registry` - The tunnel registry for managing tunnel state
    /// * `config` - Server configuration
    /// * `token_validator` - Function to validate authentication tokens
    #[must_use]
    pub fn new(
        registry: Arc<TunnelRegistry>,
        config: TunnelServerConfig,
        token_validator: TokenValidator,
    ) -> Self {
        Self {
            registry,
            config,
            token_validator,
        }
    }

    /// Handle a new WebSocket connection
    ///
    /// This is the main entry point for each tunnel client connection.
    /// It performs the following steps:
    /// 1. Upgrade TCP connection to WebSocket
    /// 2. Wait for and validate AUTH message
    /// 3. Register the tunnel in the registry
    /// 4. Run the main message loop until disconnection
    /// 5. Clean up the tunnel on disconnect
    ///
    /// # Arguments
    ///
    /// * `stream` - The TCP stream to upgrade
    /// * `client_addr` - The client's socket address
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - WebSocket upgrade fails
    /// - Authentication times out or fails
    /// - Token is invalid or already in use
    /// - Connection is closed unexpectedly
    pub async fn handle_connection(
        &self,
        stream: TcpStream,
        client_addr: SocketAddr,
    ) -> Result<()> {
        // Upgrade to WebSocket
        let ws_stream = accept_async(stream)
            .await
            .map_err(TunnelError::connection)?;

        let (mut ws_sink, mut ws_stream) = ws_stream.split();

        // Wait for AUTH message with timeout (10 seconds)
        let auth_timeout = Duration::from_secs(10);
        let auth_msg = timeout(auth_timeout, async {
            while let Some(msg) = ws_stream.next().await {
                match msg {
                    Ok(WsMessage::Binary(data)) => {
                        return Message::decode(&data).map(|(m, _)| m);
                    }
                    Ok(WsMessage::Close(_)) => {
                        return Err(TunnelError::connection_msg("Client closed connection"));
                    }
                    Ok(_) => {} // Ignore text, ping, pong
                    Err(e) => return Err(TunnelError::connection(e)),
                }
            }
            Err(TunnelError::connection_msg("Connection closed before auth"))
        })
        .await
        .map_err(|_| TunnelError::timeout())??;

        // Validate AUTH message
        let Message::Auth {
            token,
            client_id: _,
        } = auth_msg
        else {
            let fail = Message::AuthFail {
                reason: "Expected AUTH message".to_string(),
            };
            let _ = ws_sink.send(WsMessage::Binary(fail.encode().into())).await;
            return Err(TunnelError::auth("Expected AUTH message"));
        };

        // Validate token
        if let Err(e) = (self.token_validator)(&token) {
            let fail = Message::AuthFail {
                reason: e.to_string(),
            };
            let _ = ws_sink.send(WsMessage::Binary(fail.encode().into())).await;
            return Err(e);
        }

        // Hash token for storage (don't store raw token)
        let token_hash = hash_token(&token);

        // Check if token already connected
        if self.registry.token_exists(&token_hash) {
            let fail = Message::AuthFail {
                reason: "Token already in use".to_string(),
            };
            let _ = ws_sink.send(WsMessage::Binary(fail.encode().into())).await;
            return Err(TunnelError::auth("Token already in use"));
        }

        // Create control message channel
        let (control_tx, mut control_rx) = mpsc::channel::<ControlMessage>(256);

        // Register tunnel
        let tunnel = self.registry.register_tunnel(
            token_hash.clone(),
            None, // Name can be set later via REGISTER
            control_tx,
            Some(client_addr),
        )?;

        let tunnel_id = tunnel.id;

        // Send AUTH_OK
        let auth_ok = Message::AuthOk { tunnel_id };
        ws_sink
            .send(WsMessage::Binary(auth_ok.encode().into()))
            .await
            .map_err(TunnelError::connection)?;

        tracing::info!(
            tunnel_id = %tunnel_id,
            client_addr = %client_addr,
            "Tunnel authenticated"
        );

        // Main message loop
        let result = self
            .run_message_loop(tunnel_id, &mut ws_sink, &mut ws_stream, &mut control_rx)
            .await;

        // Cleanup on disconnect
        self.registry.unregister_tunnel(tunnel_id);

        tracing::info!(tunnel_id = %tunnel_id, "Tunnel disconnected");

        result
    }

    /// Run the main message loop for a connected tunnel
    ///
    /// This handles:
    /// - Heartbeat sending and timeout detection
    /// - Control messages from the registry
    /// - Incoming WebSocket messages from the client
    async fn run_message_loop(
        &self,
        tunnel_id: Uuid,
        ws_sink: &mut futures_util::stream::SplitSink<WebSocketStream<TcpStream>, WsMessage>,
        ws_stream: &mut futures_util::stream::SplitStream<WebSocketStream<TcpStream>>,
        control_rx: &mut mpsc::Receiver<ControlMessage>,
    ) -> Result<()> {
        let mut heartbeat_interval = interval(self.config.heartbeat_interval);
        let heartbeat_timeout = self.config.heartbeat_timeout;
        let mut last_heartbeat_ack = std::time::Instant::now();

        loop {
            tokio::select! {
                // Heartbeat timer
                _ = heartbeat_interval.tick() => {
                    // Check if we've received a heartbeat ack recently
                    if last_heartbeat_ack.elapsed() > heartbeat_timeout {
                        tracing::warn!(tunnel_id = %tunnel_id, "Heartbeat timeout");
                        return Err(TunnelError::timeout());
                    }

                    // Send heartbeat
                    let timestamp = current_timestamp_ms();
                    let hb = Message::Heartbeat { timestamp };
                    ws_sink
                        .send(WsMessage::Binary(hb.encode().into()))
                        .await
                        .map_err(TunnelError::connection)?;
                }

                // Control messages from registry
                Some(ctrl_msg) = control_rx.recv() => {
                    let msg = match ctrl_msg {
                        ControlMessage::Connect {
                            service_id,
                            connection_id,
                            client_addr,
                        } => Message::Connect {
                            service_id,
                            connection_id,
                            client_addr: client_addr.to_string(),
                        },
                        ControlMessage::Heartbeat { timestamp } => {
                            Message::Heartbeat { timestamp }
                        }
                        ControlMessage::Disconnect { reason } => {
                            let _ = ws_sink
                                .send(WsMessage::Binary(
                                    Message::Disconnect { reason }.encode().into(),
                                ))
                                .await;
                            return Ok(());
                        }
                    };
                    ws_sink
                        .send(WsMessage::Binary(msg.encode().into()))
                        .await
                        .map_err(TunnelError::connection)?;
                }

                // Incoming WebSocket messages
                Some(msg_result) = ws_stream.next() => {
                    match msg_result {
                        Ok(WsMessage::Binary(data)) => {
                            let (msg, _) = Message::decode(&data)?;

                            // Check for heartbeat ack before handling (so we update even if handling fails)
                            if matches!(msg, Message::HeartbeatAck { .. }) {
                                last_heartbeat_ack = std::time::Instant::now();
                            }

                            self.handle_client_message(tunnel_id, msg, ws_sink).await?;

                            // Update activity
                            self.registry.touch_tunnel(tunnel_id);
                        }
                        Ok(WsMessage::Close(_)) => {
                            return Ok(());
                        }
                        Ok(WsMessage::Ping(data)) => {
                            ws_sink
                                .send(WsMessage::Pong(data))
                                .await
                                .map_err(TunnelError::connection)?;
                        }
                        Ok(_) => {} // Ignore other message types
                        Err(e) => {
                            return Err(TunnelError::connection(e));
                        }
                    }
                }

                else => break,
            }
        }

        Ok(())
    }

    /// Handle a message from the tunnel client
    #[allow(clippy::too_many_lines)]
    async fn handle_client_message(
        &self,
        tunnel_id: Uuid,
        msg: Message,
        ws_sink: &mut futures_util::stream::SplitSink<WebSocketStream<TcpStream>, WsMessage>,
    ) -> Result<()> {
        match msg {
            Message::Register {
                name,
                protocol,
                local_port,
                remote_port,
            } => {
                self.handle_register(tunnel_id, &name, protocol, local_port, remote_port, ws_sink)
                    .await?;
            }

            Message::Unregister { service_id } => {
                if let Err(e) = self.registry.remove_service(tunnel_id, service_id) {
                    tracing::warn!(
                        tunnel_id = %tunnel_id,
                        service_id = %service_id,
                        error = %e,
                        "Service unregistration failed"
                    );
                } else {
                    tracing::info!(
                        tunnel_id = %tunnel_id,
                        service_id = %service_id,
                        "Service unregistered"
                    );
                }
            }

            Message::ConnectAck { connection_id } => {
                tracing::debug!(
                    tunnel_id = %tunnel_id,
                    connection_id = %connection_id,
                    "Connection acknowledged"
                );
                // The listener handles this via a callback
            }

            Message::ConnectFail {
                connection_id,
                reason,
            } => {
                tracing::warn!(
                    tunnel_id = %tunnel_id,
                    connection_id = %connection_id,
                    reason = %reason,
                    "Connection failed"
                );
            }

            Message::HeartbeatAck { timestamp } => {
                let now = current_timestamp_ms();
                let latency_ms = now.saturating_sub(timestamp);
                tracing::trace!(
                    tunnel_id = %tunnel_id,
                    latency_ms = latency_ms,
                    "Heartbeat ack received"
                );
            }

            // Client shouldn't send these - they're server-to-client messages
            Message::Auth { .. }
            | Message::AuthOk { .. }
            | Message::AuthFail { .. }
            | Message::RegisterOk { .. }
            | Message::RegisterFail { .. }
            | Message::Connect { .. }
            | Message::Heartbeat { .. }
            | Message::Disconnect { .. } => {
                tracing::warn!(
                    tunnel_id = %tunnel_id,
                    msg_type = ?msg.message_type(),
                    "Unexpected message from client"
                );
            }
        }

        Ok(())
    }

    /// Handle a service registration request
    async fn handle_register(
        &self,
        tunnel_id: Uuid,
        name: &str,
        protocol: ServiceProtocol,
        local_port: u16,
        remote_port: u16,
        ws_sink: &mut futures_util::stream::SplitSink<WebSocketStream<TcpStream>, WsMessage>,
    ) -> Result<()> {
        let result = self
            .registry
            .add_service(tunnel_id, name, protocol, local_port, remote_port);

        let response = match result {
            Ok(service) => {
                tracing::info!(
                    tunnel_id = %tunnel_id,
                    service_name = %name,
                    local_port = local_port,
                    remote_port = service.assigned_port.unwrap_or(remote_port),
                    "Service registered"
                );
                Message::RegisterOk {
                    service_id: service.id,
                }
            }
            Err(e) => {
                tracing::warn!(
                    tunnel_id = %tunnel_id,
                    service_name = %name,
                    error = %e,
                    "Service registration failed"
                );
                Message::RegisterFail {
                    reason: e.to_string(),
                }
            }
        };

        ws_sink
            .send(WsMessage::Binary(response.encode().into()))
            .await
            .map_err(TunnelError::connection)?;

        Ok(())
    }
}

/// Hash a token for storage (SHA256, hex encoded)
///
/// This function takes a raw authentication token and produces a secure hash
/// suitable for storage and comparison. The raw token should never be stored.
///
/// # Example
///
/// ```rust
/// use zlayer_tunnel::hash_token;
///
/// let hash = hash_token("my-secret-token");
/// assert_eq!(hash.len(), 64); // SHA256 produces 32 bytes = 64 hex chars
/// ```
#[must_use]
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

/// Simple token validator that accepts any non-empty token
///
/// This is a basic validator suitable for development or testing.
/// In production, you should implement proper token validation against
/// your authentication system.
///
/// # Errors
///
/// Returns an error if the token is empty.
///
/// # Example
///
/// ```rust
/// use zlayer_tunnel::accept_all_tokens;
///
/// assert!(accept_all_tokens("valid-token").is_ok());
/// assert!(accept_all_tokens("").is_err());
/// ```
pub fn accept_all_tokens(token: &str) -> Result<()> {
    if token.is_empty() {
        return Err(TunnelError::auth("Token cannot be empty"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_token_consistent() {
        let token = "my-secret-token";
        let hash1 = hash_token(token);
        let hash2 = hash_token(token);

        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64); // SHA256 produces 32 bytes = 64 hex chars
    }

    #[test]
    fn test_hash_token_different_tokens() {
        let hash1 = hash_token("token1");
        let hash2 = hash_token("token2");

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_hash_token_empty() {
        let hash = hash_token("");
        // Even empty string produces a valid hash
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn test_hash_token_known_value() {
        // Verify against a known SHA256 hash
        let hash = hash_token("test");
        // SHA256("test") = 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08
        assert_eq!(
            hash,
            "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
        );
    }

    #[test]
    fn test_accept_all_tokens_valid() {
        assert!(accept_all_tokens("valid-token").is_ok());
        assert!(accept_all_tokens("a").is_ok());
        assert!(accept_all_tokens("very-long-token-with-many-characters").is_ok());
    }

    #[test]
    fn test_accept_all_tokens_empty() {
        let result = accept_all_tokens("");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot be empty"));
    }

    #[test]
    fn test_control_handler_creation() {
        let registry = Arc::new(TunnelRegistry::default());
        let config = TunnelServerConfig::default();
        let validator = Arc::new(accept_all_tokens);

        let handler = ControlHandler::new(registry.clone(), config, validator);

        // Just verify it creates without panic
        assert!(Arc::strong_count(&handler.registry) >= 1);
    }

    #[test]
    fn test_hash_token_unicode() {
        // Ensure unicode tokens work correctly
        let hash = hash_token("token-with-unicode-\u{1F600}");
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn test_hash_token_special_chars() {
        let hash = hash_token("token!@#$%^&*()");
        assert_eq!(hash.len(), 64);
    }
}
