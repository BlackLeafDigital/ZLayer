//! ZLayer Overlay - Encrypted overlay networking via boringtun
//!
//! Provides encrypted overlay networks using boringtun (Cloudflare's Rust userspace
//! WireGuard implementation) with DNS service discovery, automatic bootstrap on
//! node init/join, IP allocation, and health checking.
//!
//! No kernel WireGuard module or wireguard-tools required -- uses TUN devices
//! via `/dev/net/tun` and configures peers via the UAPI protocol.
//!
//! # Modules
//!
//! - [`allocator`] - IP address allocation for overlay networks
//! - [`bootstrap`] - Overlay network initialization and joining
//! - [`config`] - Configuration types for overlay networks
//! - [`dns`] - DNS server for service discovery
//! - [`error`] - Error types for overlay operations
//! - [`health`] - Health checking for peer connectivity
//! - [`transport`] - Overlay transport (boringtun device management via UAPI)
//!
//! # Example
//!
//! ## Initialize as cluster leader
//!
//! ```ignore
//! use zlayer_overlay::bootstrap::OverlayBootstrap;
//! use std::path::Path;
//!
//! let bootstrap = OverlayBootstrap::init_leader(
//!     "10.200.0.0/16",
//!     51820,
//!     Path::new("/var/lib/zlayer"),
//! ).await?;
//!
//! // Start the overlay network (creates boringtun TUN device)
//! bootstrap.start().await?;
//!
//! println!("Overlay IP: {}", bootstrap.node_ip());
//! println!("Public key: {}", bootstrap.public_key());
//! ```
//!
//! ## Join an existing overlay
//!
//! ```ignore
//! use zlayer_overlay::bootstrap::OverlayBootstrap;
//! use std::path::Path;
//!
//! let bootstrap = OverlayBootstrap::join(
//!     "10.200.0.0/16",           // Leader's CIDR
//!     "192.168.1.100:51820",     // Leader's endpoint
//!     "leader_public_key",       // Leader's public key
//!     "10.200.0.1".parse()?,     // Leader's overlay IP
//!     "10.200.0.5".parse()?,     // Our allocated IP
//!     51820,                      // Our listen port
//!     Path::new("/var/lib/zlayer"),
//! ).await?;
//!
//! bootstrap.start().await?;
//! ```
//!
//! ## With DNS service discovery
//!
//! ```ignore
//! use zlayer_overlay::OverlayBootstrap;
//! use std::path::Path;
//!
//! // Enable DNS service discovery on the overlay
//! let mut bootstrap = OverlayBootstrap::init_leader(
//!     "10.200.0.0/16",
//!     51820,
//!     Path::new("/var/lib/zlayer"),
//! )
//! .await?
//! .with_dns("overlay.local.", 15353)?;  // Zone and port
//!
//! bootstrap.start().await?;
//!
//! // Peers are auto-registered:
//! // - node-0-1.overlay.local -> 10.200.0.1 (leader)
//! // - leader.overlay.local -> 10.200.0.1 (alias)
//!
//! // Query DNS from another machine:
//! // dig @10.200.0.1 -p 15353 node-0-1.overlay.local
//! ```
//!
//! ## Health checking
//!
//! ```ignore
//! use zlayer_overlay::health::OverlayHealthChecker;
//! use std::time::Duration;
//!
//! let checker = OverlayHealthChecker::new("zl-overlay0", Duration::from_secs(30));
//!
//! // Check all peers
//! let health = checker.check_all().await?;
//! println!("Healthy: {}/{}", health.healthy_peers, health.total_peers);
//!
//! // Start continuous monitoring
//! checker.run(|public_key, healthy| {
//!     println!("Peer {} is now {}", public_key, if healthy { "UP" } else { "DOWN" });
//! }).await;
//! ```

pub mod allocator;
pub mod bootstrap;
pub mod config;
pub mod dns;
pub mod error;
pub mod health;
pub mod transport;

// Re-export commonly used types
pub use allocator::IpAllocator;
pub use bootstrap::{
    BootstrapConfig, BootstrapState, OverlayBootstrap, PeerConfig, DEFAULT_INTERFACE_NAME,
    DEFAULT_KEEPALIVE_SECS, DEFAULT_OVERLAY_CIDR, DEFAULT_WG_PORT,
};
pub use config::*;
pub use dns::*;
pub use error::{OverlayError, Result};
pub use health::{OverlayHealth, OverlayHealthChecker, PeerStatus};
pub use transport::*;
