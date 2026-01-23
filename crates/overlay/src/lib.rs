//! ZLayer Overlay - WireGuard-based encrypted networking
//!
//! Provides kernel WireGuard overlay networks with DNS service discovery.

pub mod config;
pub mod dns;
pub mod wireguard;

pub use config::*;
pub use dns::*;
pub use wireguard::*;
