//! Tunnel client components
//!
//! This module provides client-side components for connecting to tunnel servers,
//! including the [`TunnelAgent`] for establishing and maintaining tunnel connections,
//! service registration, and handling incoming connections.

pub mod agent;
pub mod proxy;

pub use agent::*;
pub use proxy::*;
