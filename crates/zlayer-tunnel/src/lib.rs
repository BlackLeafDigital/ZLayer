//! `ZLayer` Tunnel - Secure tunneling for `ZLayer` services
//!
//! Provides secure tunnel functionality for accessing `ZLayer` services
//! through authenticated tunnels, including:
//!
//! - **SSH to containers** - Tunnel SSH access to specific containers without exposing overlay network
//! - **Database access** - Securely expose PostgreSQL/MySQL through tunnel with auth
//! - **Game server tunneling** - TCP/UDP tunneling for game servers
//! - **Node-to-node bridging** - Connect `ZLayer` nodes across different networks/datacenters
//! - **On-demand access** - Like Cloudflare Access, users request temporary access to hidden services via CLI
//!
//! # Architecture
//!
//! The tunnel system consists of:
//!
//! - **Control Channel**: WebSocket over TLS for authentication and coordination
//! - **Data Channels**: Direct TCP/UDP connections for actual traffic
//! - **Registry**: Tracks active tunnels and their services
//!
//! # Protocol
//!
//! Uses a compact binary message format:
//!
//! ```text
//! +----------+----------+----------------------------------+
//! | Type(1)  | Len(4)   | Payload (variable)               |
//! +----------+----------+----------------------------------+
//! ```
//!
//! # Example
//!
//! ```rust,no_run
//! use zlayer_tunnel::{TunnelClientConfig, ServiceConfig};
//!
//! // Create a client configuration
//! let config = TunnelClientConfig::new(
//!     "wss://tunnel.example.com/tunnel/v1",
//!     "tun_abc123"
//! )
//! .with_service(ServiceConfig::tcp("ssh", 22).with_remote_port(2222))
//! .with_service(ServiceConfig::tcp("postgres", 5432));
//!
//! // Validate the configuration
//! config.validate().expect("invalid config");
//! ```

#![deny(unsafe_code)]
#![warn(missing_docs)]
#![warn(clippy::all)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

pub mod access;
pub mod client;
pub mod config;
pub mod error;
pub mod node;
pub mod protocol;
pub mod server;

// Re-export main types at crate root
pub use access::{AccessManager, AccessSession, SessionInfo};
pub use client::{
    AgentState, ConnectionCallback, ControlCommand, ControlEvent, LocalProxy, RegisteredService,
    ServiceStatus, TunnelAgent,
};
pub use config::{ServiceConfig, TunnelClientConfig, TunnelServerConfig};
pub use error::{Result, TunnelError};
pub use node::{NodeTunnel, NodeTunnelManager, TunnelState, TunnelStatus};
pub use protocol::{
    Message, MessageType, ServiceProtocol, HEADER_SIZE, MAX_MESSAGE_SIZE, PROTOCOL_VERSION,
};
pub use server::{
    accept_all_tokens, hash_token, ControlHandler, ControlMessage, ListenerManager, TokenValidator,
    Tunnel, TunnelInfo, TunnelRegistry, TunnelService,
};
pub use zlayer_spec::ExposeType;
