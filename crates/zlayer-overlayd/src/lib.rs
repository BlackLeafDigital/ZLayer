//! `zlayer-overlayd` — the standalone ZLayer overlay daemon.
//!
//! This crate is the long-lived process that owns every mechanism touching the
//! overlay/network plane: the `WireGuard`/Wintun device + adapter, peers,
//! `AllowedIPs`/service subnets, IP allocation, DNS, NAT, Linux bridges +
//! veth/netns attach, and the Windows HCN Internal network + endpoints. The
//! main `zlayer` daemon keeps the cluster brain and drives overlayd over the
//! IPC contract in [`zlayer_types::overlayd`].
//!
//! Running overlayd as its own OS service decouples the overlay adapter's
//! lifetime from the main binary: updating/reinstalling `zlayer` no longer
//! tears the adapter down, because overlayd is a separate process. The overlay
//! is removed only on a full uninstall.
//!
//! ## Layout
//! - [`transport`] — length-prefixed JSON framing over UDS / named pipe.
//! - [`client`] — [`OverlaydClient`], used by the main daemon (the agent's
//!   `overlay_manager` shim wraps it).
//! - the server engine (wrapping the `zlayer-overlay` `OverlayBootstrap` plus
//!   the migrated bridge/HCN/IPAM mechanics) lands in a follow-up module.

pub mod client;
pub mod error;
pub mod transport;

pub use client::OverlaydClient;
pub use error::{OverlaydError, Result, MAX_FRAME_BYTES};

/// The IPC wire contract, re-exported for convenience.
pub use zlayer_types::overlayd as protocol;
