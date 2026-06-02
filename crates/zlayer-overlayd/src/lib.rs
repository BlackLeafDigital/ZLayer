//! `zlayer-overlayd` — the standalone `ZLayer` overlay daemon.
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
//! - [`server`] — [`OverlaydServer`], the engine that executes every
//!   [`OverlaydRequest`] by performing the same overlay mechanics the agent's
//!   `OverlayManager` did (cluster `WireGuard` transport, Linux bridges +
//!   veth/netns attach, Windows HCN Internal network + endpoints, IPAM, DNS,
//!   NAT).
//! - [`network_state`] — the on-disk marker for host-level networks (HCN).
//! - [`netlink`] — Linux RTNETLINK helpers for bridges, veth, routes, netns.
//!
//! [`OverlaydRequest`]: zlayer_types::overlayd::OverlaydRequest

pub mod client;
pub mod error;
pub mod network_state;
pub mod server;
pub mod transport;

#[cfg(target_os = "linux")]
pub mod netlink;

pub use client::OverlaydClient;
pub use error::{OverlaydError, Result, MAX_FRAME_BYTES};
pub use server::OverlaydServer;

/// The IPC wire contract, re-exported for convenience.
pub use zlayer_types::overlayd as protocol;
