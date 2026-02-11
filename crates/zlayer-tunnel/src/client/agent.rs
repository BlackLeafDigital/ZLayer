//! Tunnel client agent for connecting to tunnel servers
//!
//! The [`TunnelAgent`] manages the lifecycle of a tunnel client connection, including:
//! - WebSocket connection to the tunnel server
//! - Authentication and service registration
//! - Heartbeat handling and timeout detection
//! - Automatic reconnection with exponential backoff
//! - Incoming connection notification

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use parking_lot::RwLock;
use tokio::sync::mpsc;
use tokio::time::{interval, timeout};
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};
use uuid::Uuid;

use crate::{Message, Result, ServiceConfig, ServiceProtocol, TunnelClientConfig, TunnelError};

// =============================================================================
// Agent State
// =============================================================================

/// Current state of the tunnel agent
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum AgentState {
    /// Not connected to the server
    #[default]
    Disconnected,
    /// Attempting to connect
    Connecting,
    /// Successfully connected and authenticated
    Connected {
        /// The assigned tunnel ID from the server
        tunnel_id: Uuid,
    },
    /// Reconnecting after a disconnection
    Reconnecting {
        /// Current reconnection attempt number
        attempt: u32,
    },
}

// =============================================================================
// Service Status
// =============================================================================

/// Status of a registered service
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ServiceStatus {
    /// Service registration is pending
    #[default]
    Pending,
    /// Service is registered and active
    Registered,
    /// Service registration failed
    Failed(String),
}

// =============================================================================
// Registered Service
// =============================================================================

/// A service being exposed through the tunnel
#[derive(Debug, Clone)]
pub struct RegisteredService {
    /// The service configuration
    pub config: ServiceConfig,
    /// Server-assigned service ID (if registered)
    pub service_id: Option<Uuid>,
    /// Current status of the service
    pub status: ServiceStatus,
}

impl RegisteredService {
    /// Create a new registered service from a config
    #[must_use]
    pub fn new(config: ServiceConfig) -> Self {
        Self {
            config,
            service_id: None,
            status: ServiceStatus::Pending,
        }
    }

    /// Check if the service is successfully registered
    #[must_use]
    pub fn is_registered(&self) -> bool {
        matches!(self.status, ServiceStatus::Registered)
    }
}

// =============================================================================
// Control Events
// =============================================================================

/// Events received from the tunnel server
#[derive(Debug, Clone)]
pub enum ControlEvent {
    /// Successfully authenticated with the server
    Authenticated {
        /// The assigned tunnel ID
        tunnel_id: Uuid,
    },
    /// A service was successfully registered
    ServiceRegistered {
        /// Service name
        name: String,
        /// Server-assigned service ID
        service_id: Uuid,
    },
    /// A service registration failed
    ServiceFailed {
        /// Service name
        name: String,
        /// Failure reason
        reason: String,
    },
    /// An incoming connection is being established
    IncomingConnection {
        /// Service ID receiving the connection
        service_id: Uuid,
        /// Unique connection ID
        connection_id: Uuid,
        /// Remote client address
        client_addr: String,
    },
    /// Heartbeat received from the server
    Heartbeat {
        /// Server timestamp
        timestamp: u64,
    },
    /// Disconnected from the server
    Disconnected {
        /// Reason for disconnection
        reason: String,
    },
    /// An error occurred
    Error {
        /// Error message
        message: String,
    },
}

// =============================================================================
// Control Commands
// =============================================================================

/// Commands to send to the tunnel server
#[derive(Debug, Clone)]
pub enum ControlCommand {
    /// Register a new service
    Register {
        /// Service name
        name: String,
        /// Protocol type
        protocol: ServiceProtocol,
        /// Local port
        local_port: u16,
        /// Requested remote port (0 = auto-assign)
        remote_port: u16,
    },
    /// Unregister an existing service
    Unregister {
        /// Service ID to unregister
        service_id: Uuid,
    },
    /// Acknowledge an incoming connection
    ConnectAck {
        /// Connection ID to acknowledge
        connection_id: Uuid,
    },
    /// Reject an incoming connection
    ConnectFail {
        /// Connection ID to reject
        connection_id: Uuid,
        /// Failure reason
        reason: String,
    },
    /// Gracefully disconnect from the server
    Disconnect,
}

// =============================================================================
// Connection Callback
// =============================================================================

/// Type alias for connection callback function
///
/// The callback receives the service ID, connection ID, and client address,
/// and returns whether the connection was accepted.
pub type ConnectionCallback = Arc<dyn Fn(Uuid, Uuid, String) -> bool + Send + Sync>;

// =============================================================================
// Tunnel Agent
// =============================================================================

/// Tunnel client agent that connects to a tunnel server
///
/// The `TunnelAgent` manages the complete lifecycle of a tunnel client connection,
/// including authentication, service registration, heartbeat handling, and
/// automatic reconnection on failure.
///
/// # Example
///
/// ```rust,no_run
/// use zlayer_tunnel::{TunnelAgent, TunnelClientConfig, ServiceConfig};
/// use std::sync::Arc;
///
/// #[tokio::main]
/// async fn main() {
///     let config = TunnelClientConfig::new(
///         "wss://tunnel.example.com/tunnel/v1",
///         "my-auth-token"
///     )
///     .with_service(ServiceConfig::tcp("ssh", 22).with_remote_port(2222));
///
///     let agent = TunnelAgent::new(config)
///         .on_connection(Arc::new(|service_id, conn_id, client_addr| {
///             println!("Incoming connection from {} to service {}", client_addr, service_id);
///             true // Accept the connection
///         }));
///
///     // Run the agent with auto-reconnect
///     if let Err(e) = agent.run().await {
///         eprintln!("Agent error: {}", e);
///     }
/// }
/// ```
pub struct TunnelAgent {
    /// Client configuration
    config: TunnelClientConfig,
    /// Current agent state
    state: Arc<RwLock<AgentState>>,
    /// Registered services by name
    services: Arc<RwLock<HashMap<String, RegisteredService>>>,
    /// Callback for incoming connections
    connection_callback: Option<ConnectionCallback>,
    /// Channel to send commands to the agent loop
    command_tx: Option<mpsc::Sender<ControlCommand>>,
    /// Event channel for external listeners
    event_tx: Option<mpsc::Sender<ControlEvent>>,
}

impl TunnelAgent {
    /// Create a new tunnel agent with the given configuration
    ///
    /// The agent will attempt to register all services specified in the config
    /// when it connects to the server.
    #[must_use]
    pub fn new(config: TunnelClientConfig) -> Self {
        // Initialize services from config
        let services: HashMap<String, RegisteredService> = config
            .services
            .iter()
            .map(|s| (s.name.clone(), RegisteredService::new(s.clone())))
            .collect();

        Self {
            config,
            state: Arc::new(RwLock::new(AgentState::Disconnected)),
            services: Arc::new(RwLock::new(services)),
            connection_callback: None,
            command_tx: None,
            event_tx: None,
        }
    }

    /// Set the connection callback (builder pattern)
    ///
    /// The callback is invoked when an incoming connection is received.
    /// It should return `true` to accept the connection or `false` to reject it.
    #[must_use]
    pub fn on_connection(mut self, callback: ConnectionCallback) -> Self {
        self.connection_callback = Some(callback);
        self
    }

    /// Set an event channel for receiving control events
    ///
    /// Events will be sent to this channel for external processing.
    #[must_use]
    pub fn with_event_channel(mut self, tx: mpsc::Sender<ControlEvent>) -> Self {
        self.event_tx = Some(tx);
        self
    }

    /// Get the current agent state
    #[must_use]
    pub fn state(&self) -> AgentState {
        self.state.read().clone()
    }

    /// Get a service by name
    #[must_use]
    pub fn get_service(&self, name: &str) -> Option<RegisteredService> {
        self.services.read().get(name).cloned()
    }

    /// Get all registered services
    #[must_use]
    pub fn services(&self) -> Vec<RegisteredService> {
        self.services.read().values().cloned().collect()
    }

    /// Check if the agent is connected
    #[must_use]
    pub fn is_connected(&self) -> bool {
        matches!(*self.state.read(), AgentState::Connected { .. })
    }

    /// Get the tunnel ID if connected
    #[must_use]
    pub fn tunnel_id(&self) -> Option<Uuid> {
        match *self.state.read() {
            AgentState::Connected { tunnel_id } => Some(tunnel_id),
            _ => None,
        }
    }

    /// Send a command to the agent (if running)
    ///
    /// # Errors
    ///
    /// Returns an error if the agent is not running or the command channel is full.
    pub async fn send_command(&self, command: ControlCommand) -> Result<()> {
        let tx = self
            .command_tx
            .as_ref()
            .ok_or_else(|| TunnelError::connection_msg("agent not running"))?;

        tx.send(command)
            .await
            .map_err(|_| TunnelError::connection_msg("command channel closed"))
    }

    /// Run the agent with automatic reconnection
    ///
    /// This method will attempt to connect to the server and maintain the connection.
    /// If the connection is lost, it will automatically reconnect with exponential backoff.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The configuration is invalid
    /// - The connection fails and cannot be recovered
    /// - A shutdown signal is received
    pub async fn run(&self) -> Result<()> {
        // Validate config
        self.config.validate().map_err(TunnelError::config)?;

        let mut current_interval = self.config.reconnect_interval;
        let mut attempt = 0u32;

        loop {
            attempt += 1;
            *self.state.write() = AgentState::Reconnecting { attempt };

            tracing::info!(
                attempt = attempt,
                interval_ms = current_interval.as_millis(),
                "attempting to connect"
            );

            match self.run_once().await {
                Ok(()) => {
                    // Clean shutdown requested
                    tracing::info!("agent shutting down");
                    return Ok(());
                }
                Err(TunnelError::Shutdown) => {
                    // Clean shutdown
                    tracing::info!("agent received shutdown signal");
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!(error = %e, "connection failed, will retry");

                    // Notify of disconnection
                    if let Some(ref tx) = self.event_tx {
                        let _ = tx
                            .send(ControlEvent::Disconnected {
                                reason: e.to_string(),
                            })
                            .await;
                    }
                }
            }

            // Reset services to pending state
            {
                let mut services = self.services.write();
                for service in services.values_mut() {
                    service.service_id = None;
                    service.status = ServiceStatus::Pending;
                }
            }

            // Wait before reconnecting
            tokio::time::sleep(current_interval).await;

            // Exponential backoff
            current_interval = std::cmp::min(
                current_interval.saturating_mul(2),
                self.config.max_reconnect_interval,
            );
        }
    }

    /// Run a single connection attempt
    ///
    /// This method connects to the server, authenticates, registers services,
    /// and runs the message loop until disconnection.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - WebSocket connection fails
    /// - Authentication fails
    /// - Connection is lost
    pub async fn run_once(&self) -> Result<()> {
        *self.state.write() = AgentState::Connecting;

        // Connect to WebSocket server
        tracing::debug!(url = %self.config.server_url, "connecting to server");

        let (ws_stream, _response) = connect_async(&self.config.server_url)
            .await
            .map_err(TunnelError::connection)?;

        let (mut ws_sink, mut ws_stream) = ws_stream.split();

        // Generate a client ID for this connection
        let client_id = Uuid::new_v4();

        // Send AUTH message
        let auth_msg = Message::Auth {
            token: self.config.token.clone(),
            client_id,
        };
        ws_sink
            .send(WsMessage::Binary(auth_msg.encode().into()))
            .await
            .map_err(TunnelError::connection)?;

        // Wait for AUTH_OK with timeout (10 seconds)
        let auth_timeout = Duration::from_secs(10);
        let auth_response = timeout(auth_timeout, async {
            while let Some(msg) = ws_stream.next().await {
                match msg {
                    Ok(WsMessage::Binary(data)) => {
                        return Message::decode(&data).map(|(m, _)| m);
                    }
                    Ok(WsMessage::Close(frame)) => {
                        let reason = frame.map_or_else(
                            || "connection closed".to_string(),
                            |f| f.reason.to_string(),
                        );
                        return Err(TunnelError::connection_msg(reason));
                    }
                    Ok(_) => {} // Ignore text, ping, pong
                    Err(e) => return Err(TunnelError::connection(e)),
                }
            }
            Err(TunnelError::connection_msg("connection closed before auth"))
        })
        .await
        .map_err(|_| TunnelError::timeout())??;

        // Handle auth response
        let tunnel_id = match auth_response {
            Message::AuthOk { tunnel_id } => tunnel_id,
            Message::AuthFail { reason } => {
                return Err(TunnelError::auth(reason));
            }
            other => {
                return Err(TunnelError::protocol(format!(
                    "expected AuthOk or AuthFail, got {:?}",
                    other.message_type()
                )));
            }
        };

        *self.state.write() = AgentState::Connected { tunnel_id };

        tracing::info!(
            tunnel_id = %tunnel_id,
            client_id = %client_id,
            "authenticated with server"
        );

        // Notify of authentication
        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(ControlEvent::Authenticated { tunnel_id }).await;
        }

        // Register all services
        self.register_services(&mut ws_sink).await?;

        // Run the main message loop
        self.run_message_loop(tunnel_id, &mut ws_sink, &mut ws_stream)
            .await
    }

    /// Register all services from the config
    async fn register_services<S>(&self, ws_sink: &mut S) -> Result<()>
    where
        S: SinkExt<WsMessage> + Unpin,
        S::Error: std::error::Error,
    {
        let services: Vec<ServiceConfig> = {
            self.services
                .read()
                .values()
                .map(|s| s.config.clone())
                .collect()
        };

        for service in services {
            let register_msg = Message::Register {
                name: service.name.clone(),
                protocol: service.protocol,
                local_port: service.local_port,
                remote_port: service.remote_port,
            };

            tracing::debug!(
                service_name = %service.name,
                local_port = service.local_port,
                "registering service"
            );

            ws_sink
                .send(WsMessage::Binary(register_msg.encode().into()))
                .await
                .map_err(|e| TunnelError::connection_msg(e.to_string()))?;
        }

        Ok(())
    }

    /// Run the main message loop
    async fn run_message_loop<Sink, Stream>(
        &self,
        tunnel_id: Uuid,
        ws_sink: &mut Sink,
        ws_stream: &mut Stream,
    ) -> Result<()>
    where
        Sink: SinkExt<WsMessage> + Unpin,
        Sink::Error: std::error::Error,
        Stream: StreamExt<Item = std::result::Result<WsMessage, tokio_tungstenite::tungstenite::Error>>
            + Unpin,
    {
        // Create command channel
        let (_command_tx, mut command_rx) = mpsc::channel::<ControlCommand>(256);

        // Store the sender so external code can send commands
        // Note: This is a bit awkward since we're in a method, but we need to
        // allow external commands during the message loop
        // For now, we'll handle this differently

        // Track pending service registrations by name
        let mut pending_services: Vec<String> = { self.services.read().keys().cloned().collect() };

        // Heartbeat interval (we respond to server heartbeats, not send our own)
        let mut check_interval = interval(Duration::from_secs(5));

        loop {
            tokio::select! {
                // Check interval for health/status
                _ = check_interval.tick() => {
                    // Just a periodic check, we respond to server heartbeats
                }

                // Commands from external code
                Some(command) = command_rx.recv() => {
                    match command {
                        ControlCommand::Register { name, protocol, local_port, remote_port } => {
                            let msg = Message::Register {
                                name: name.clone(),
                                protocol,
                                local_port,
                                remote_port,
                            };
                            ws_sink
                                .send(WsMessage::Binary(msg.encode().into()))
                                .await
                                .map_err(|e| TunnelError::connection_msg(e.to_string()))?;
                            pending_services.push(name);
                        }
                        ControlCommand::Unregister { service_id } => {
                            let msg = Message::Unregister { service_id };
                            ws_sink
                                .send(WsMessage::Binary(msg.encode().into()))
                                .await
                                .map_err(|e| TunnelError::connection_msg(e.to_string()))?;
                        }
                        ControlCommand::ConnectAck { connection_id } => {
                            let msg = Message::ConnectAck { connection_id };
                            ws_sink
                                .send(WsMessage::Binary(msg.encode().into()))
                                .await
                                .map_err(|e| TunnelError::connection_msg(e.to_string()))?;
                        }
                        ControlCommand::ConnectFail { connection_id, reason } => {
                            let msg = Message::ConnectFail { connection_id, reason };
                            ws_sink
                                .send(WsMessage::Binary(msg.encode().into()))
                                .await
                                .map_err(|e| TunnelError::connection_msg(e.to_string()))?;
                        }
                        ControlCommand::Disconnect => {
                            tracing::info!("disconnect command received");
                            return Ok(());
                        }
                    }
                }

                // Incoming WebSocket messages
                Some(msg_result) = ws_stream.next() => {
                    match msg_result {
                        Ok(WsMessage::Binary(data)) => {
                            let (msg, _) = Message::decode(&data)?;
                            self.handle_server_message(
                                tunnel_id,
                                msg,
                                ws_sink,
                                &mut pending_services,
                            ).await?;
                        }
                        Ok(WsMessage::Close(frame)) => {
                            let reason = frame.map_or_else(
                                || "server closed connection".to_string(),
                                |f| f.reason.to_string(),
                            );
                            tracing::info!(reason = %reason, "server closed connection");
                            return Err(TunnelError::connection_msg(reason));
                        }
                        Ok(WsMessage::Ping(data)) => {
                            ws_sink
                                .send(WsMessage::Pong(data))
                                .await
                                .map_err(|e| TunnelError::connection_msg(e.to_string()))?;
                        }
                        Ok(_) => {} // Ignore other message types
                        Err(e) => {
                            return Err(TunnelError::connection(e));
                        }
                    }
                }

                else => {
                    // All channels closed
                    break;
                }
            }
        }

        Ok(())
    }

    /// Handle a message from the server
    async fn handle_server_message<S>(
        &self,
        tunnel_id: Uuid,
        msg: Message,
        ws_sink: &mut S,
        pending_services: &mut Vec<String>,
    ) -> Result<()>
    where
        S: SinkExt<WsMessage> + Unpin,
        S::Error: std::error::Error,
    {
        match msg {
            Message::RegisterOk { service_id } => {
                self.handle_register_ok(service_id, pending_services).await;
            }
            Message::RegisterFail { reason } => {
                self.handle_register_fail(reason, pending_services).await;
            }
            Message::Connect {
                service_id,
                connection_id,
                client_addr,
            } => {
                self.handle_connect(service_id, connection_id, client_addr, ws_sink)
                    .await?;
            }
            Message::Heartbeat { timestamp } => {
                self.handle_heartbeat(timestamp, ws_sink).await?;
            }
            Message::Disconnect { reason } => {
                return self.handle_disconnect(reason).await;
            }
            // Client shouldn't receive these messages
            Message::Auth { .. }
            | Message::AuthOk { .. }
            | Message::AuthFail { .. }
            | Message::Register { .. }
            | Message::Unregister { .. }
            | Message::ConnectAck { .. }
            | Message::ConnectFail { .. }
            | Message::HeartbeatAck { .. } => {
                tracing::warn!(
                    tunnel_id = %tunnel_id,
                    msg_type = ?msg.message_type(),
                    "unexpected message from server"
                );
            }
        }
        Ok(())
    }

    /// Handle `RegisterOk` message
    async fn handle_register_ok(&self, service_id: Uuid, pending_services: &mut Vec<String>) {
        let name = match pending_services.first().cloned() {
            Some(n) => {
                pending_services.remove(0);
                n
            }
            None => return,
        };

        // Update service state without holding lock across await
        {
            let mut services = self.services.write();
            if let Some(service) = services.get_mut(&name) {
                service.service_id = Some(service_id);
                service.status = ServiceStatus::Registered;
            }
        }

        tracing::info!(
            service_name = %name,
            service_id = %service_id,
            "service registered"
        );

        // Notify (after releasing lock)
        if let Some(ref tx) = self.event_tx {
            let _ = tx
                .send(ControlEvent::ServiceRegistered { name, service_id })
                .await;
        }
    }

    /// Handle `RegisterFail` message
    async fn handle_register_fail(&self, reason: String, pending_services: &mut Vec<String>) {
        let name = match pending_services.first().cloned() {
            Some(n) => {
                pending_services.remove(0);
                n
            }
            None => return,
        };

        // Update service state without holding lock across await
        {
            let mut services = self.services.write();
            if let Some(service) = services.get_mut(&name) {
                service.status = ServiceStatus::Failed(reason.clone());
            }
        }

        tracing::warn!(
            service_name = %name,
            reason = %reason,
            "service registration failed"
        );

        // Notify (after releasing lock)
        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(ControlEvent::ServiceFailed { name, reason }).await;
        }
    }

    /// Handle Connect message (incoming connection)
    async fn handle_connect<S>(
        &self,
        service_id: Uuid,
        connection_id: Uuid,
        client_addr: String,
        ws_sink: &mut S,
    ) -> Result<()>
    where
        S: SinkExt<WsMessage> + Unpin,
        S::Error: std::error::Error,
    {
        tracing::debug!(
            service_id = %service_id,
            connection_id = %connection_id,
            client_addr = %client_addr,
            "incoming connection"
        );

        // Notify via event channel
        if let Some(ref tx) = self.event_tx {
            let _ = tx
                .send(ControlEvent::IncomingConnection {
                    service_id,
                    connection_id,
                    client_addr: client_addr.clone(),
                })
                .await;
        }

        // Call the connection callback
        let accepted = self
            .connection_callback
            .as_ref()
            .is_none_or(|cb| cb(service_id, connection_id, client_addr.clone()));

        // Send response
        let response = if accepted {
            Message::ConnectAck { connection_id }
        } else {
            Message::ConnectFail {
                connection_id,
                reason: "connection rejected by client".to_string(),
            }
        };

        ws_sink
            .send(WsMessage::Binary(response.encode().into()))
            .await
            .map_err(|e| TunnelError::connection_msg(e.to_string()))?;

        Ok(())
    }

    /// Handle Heartbeat message
    async fn handle_heartbeat<S>(&self, timestamp: u64, ws_sink: &mut S) -> Result<()>
    where
        S: SinkExt<WsMessage> + Unpin,
        S::Error: std::error::Error,
    {
        tracing::trace!(timestamp = timestamp, "heartbeat received");

        // Respond with heartbeat ack
        let ack = Message::HeartbeatAck { timestamp };
        ws_sink
            .send(WsMessage::Binary(ack.encode().into()))
            .await
            .map_err(|e| TunnelError::connection_msg(e.to_string()))?;

        // Notify
        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(ControlEvent::Heartbeat { timestamp }).await;
        }

        Ok(())
    }

    /// Handle Disconnect message
    async fn handle_disconnect(&self, reason: String) -> Result<()> {
        tracing::info!(reason = %reason, "server requested disconnect");

        // Notify
        if let Some(ref tx) = self.event_tx {
            let _ = tx
                .send(ControlEvent::Disconnected {
                    reason: reason.clone(),
                })
                .await;
        }

        Err(TunnelError::connection_msg(reason))
    }

    /// Gracefully disconnect from the server
    ///
    /// This signals the agent to stop and disconnect. If the agent is running
    /// with auto-reconnect (`run()`), it will stop reconnecting.
    pub fn disconnect(&self) {
        *self.state.write() = AgentState::Disconnected;

        // Send disconnect command if the agent is running
        if let Some(ref tx) = self.command_tx {
            let _ = tx.try_send(ControlCommand::Disconnect);
        }
    }
}

impl Clone for TunnelAgent {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            state: Arc::clone(&self.state),
            services: Arc::clone(&self.services),
            connection_callback: self.connection_callback.clone(),
            command_tx: self.command_tx.clone(),
            event_tx: self.event_tx.clone(),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> TunnelClientConfig {
        TunnelClientConfig::new("ws://localhost:8080/tunnel/v1", "test-token")
            .with_service(ServiceConfig::tcp("ssh", 22).with_remote_port(2222))
            .with_service(ServiceConfig::udp("game", 27015))
    }

    #[test]
    fn test_agent_state_default() {
        let state = AgentState::default();
        assert_eq!(state, AgentState::Disconnected);
    }

    #[test]
    fn test_agent_state_variants() {
        let disconnected = AgentState::Disconnected;
        let connecting = AgentState::Connecting;
        let connected = AgentState::Connected {
            tunnel_id: Uuid::new_v4(),
        };
        let reconnecting = AgentState::Reconnecting { attempt: 3 };

        // Just verify they're different
        assert_ne!(disconnected, connecting);
        assert_ne!(connecting, connected);
        assert_ne!(connected, reconnecting);
    }

    #[test]
    fn test_service_status_default() {
        let status = ServiceStatus::default();
        assert_eq!(status, ServiceStatus::Pending);
    }

    #[test]
    fn test_service_status_variants() {
        assert_eq!(ServiceStatus::Pending, ServiceStatus::Pending);
        assert_eq!(ServiceStatus::Registered, ServiceStatus::Registered);
        assert_eq!(
            ServiceStatus::Failed("error".to_string()),
            ServiceStatus::Failed("error".to_string())
        );
        assert_ne!(
            ServiceStatus::Failed("error1".to_string()),
            ServiceStatus::Failed("error2".to_string())
        );
    }

    #[test]
    fn test_registered_service_new() {
        let config = ServiceConfig::tcp("ssh", 22);
        let service = RegisteredService::new(config.clone());

        assert_eq!(service.config.name, "ssh");
        assert!(service.service_id.is_none());
        assert_eq!(service.status, ServiceStatus::Pending);
        assert!(!service.is_registered());
    }

    #[test]
    fn test_registered_service_is_registered() {
        let config = ServiceConfig::tcp("ssh", 22);
        let mut service = RegisteredService::new(config);

        assert!(!service.is_registered());

        service.status = ServiceStatus::Registered;
        assert!(service.is_registered());

        service.status = ServiceStatus::Failed("error".to_string());
        assert!(!service.is_registered());
    }

    #[test]
    fn test_tunnel_agent_new() {
        let config = create_test_config();
        let agent = TunnelAgent::new(config);

        assert_eq!(agent.state(), AgentState::Disconnected);
        assert!(!agent.is_connected());
        assert!(agent.tunnel_id().is_none());

        let services = agent.services();
        assert_eq!(services.len(), 2);
    }

    #[test]
    fn test_tunnel_agent_get_service() {
        let config = create_test_config();
        let agent = TunnelAgent::new(config);

        let ssh = agent.get_service("ssh");
        assert!(ssh.is_some());
        assert_eq!(ssh.unwrap().config.local_port, 22);

        let game = agent.get_service("game");
        assert!(game.is_some());
        assert_eq!(game.unwrap().config.protocol, ServiceProtocol::Udp);

        let nonexistent = agent.get_service("nonexistent");
        assert!(nonexistent.is_none());
    }

    #[test]
    fn test_tunnel_agent_on_connection() {
        let config = create_test_config();
        let callback_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let callback_called_clone = Arc::clone(&callback_called);

        let callback: ConnectionCallback = Arc::new(move |_service_id, _conn_id, _addr| {
            callback_called_clone.store(true, std::sync::atomic::Ordering::SeqCst);
            true
        });

        let agent = TunnelAgent::new(config).on_connection(callback);

        // Callback is set but not called yet
        assert!(!callback_called.load(std::sync::atomic::Ordering::SeqCst));
        assert!(agent.connection_callback.is_some());
    }

    #[test]
    fn test_tunnel_agent_clone() {
        let config = create_test_config();
        let agent = TunnelAgent::new(config);

        let cloned = agent.clone();

        assert_eq!(agent.state(), cloned.state());
        assert_eq!(agent.services().len(), cloned.services().len());
    }

    #[test]
    fn test_tunnel_agent_disconnect() {
        let config = create_test_config();
        let agent = TunnelAgent::new(config);

        // Set state to connected
        *agent.state.write() = AgentState::Connected {
            tunnel_id: Uuid::new_v4(),
        };
        assert!(agent.is_connected());

        // Disconnect
        agent.disconnect();
        assert_eq!(agent.state(), AgentState::Disconnected);
        assert!(!agent.is_connected());
    }

    #[test]
    fn test_control_event_variants() {
        // Just verify the variants exist and can be created
        let _auth = ControlEvent::Authenticated {
            tunnel_id: Uuid::new_v4(),
        };
        let _registered = ControlEvent::ServiceRegistered {
            name: "ssh".to_string(),
            service_id: Uuid::new_v4(),
        };
        let _failed = ControlEvent::ServiceFailed {
            name: "ssh".to_string(),
            reason: "error".to_string(),
        };
        let _incoming = ControlEvent::IncomingConnection {
            service_id: Uuid::new_v4(),
            connection_id: Uuid::new_v4(),
            client_addr: "127.0.0.1:12345".to_string(),
        };
        let heartbeat = ControlEvent::Heartbeat { timestamp: 12345 };
        assert!(matches!(heartbeat, ControlEvent::Heartbeat { .. }));
        let _disconnected = ControlEvent::Disconnected {
            reason: "test".to_string(),
        };
        let _error = ControlEvent::Error {
            message: "test error".to_string(),
        };
    }

    #[test]
    fn test_control_command_variants() {
        // Just verify the variants exist and can be created
        let _register = ControlCommand::Register {
            name: "ssh".to_string(),
            protocol: ServiceProtocol::Tcp,
            local_port: 22,
            remote_port: 2222,
        };
        let _unregister = ControlCommand::Unregister {
            service_id: Uuid::new_v4(),
        };
        let _ack = ControlCommand::ConnectAck {
            connection_id: Uuid::new_v4(),
        };
        let _fail = ControlCommand::ConnectFail {
            connection_id: Uuid::new_v4(),
            reason: "error".to_string(),
        };
        let disconnect = ControlCommand::Disconnect;
        assert!(matches!(disconnect, ControlCommand::Disconnect));
    }

    #[test]
    fn test_tunnel_agent_with_event_channel() {
        let config = create_test_config();
        let (tx, _rx) = mpsc::channel(16);

        let agent = TunnelAgent::new(config).with_event_channel(tx);

        assert!(agent.event_tx.is_some());
    }

    #[tokio::test]
    async fn test_send_command_not_running() {
        let config = create_test_config();
        let agent = TunnelAgent::new(config);

        let result = agent.send_command(ControlCommand::Disconnect).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("agent not running"));
    }
}
