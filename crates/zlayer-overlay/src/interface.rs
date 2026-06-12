//! Platform-abstracted network interface configuration.
//!
//! This module defines the [`InterfaceOps`] trait that hides the
//! host-specific mechanism used to configure the TUN interface for the
//! overlay transport. On Linux the implementation talks RTNETLINK via
//! [`crate::netlink`]; on macOS it shells out to `ifconfig` / `route`;
//! on Windows it calls IP Helper (`CreateUnicastIpAddressEntry` /
//! `CreateIpForwardEntry2`) via the `windows-rs` bindings.
//!
//! Use [`platform_ops`] to obtain the correct implementation for the
//! current host.

use std::net::IpAddr;

use async_trait::async_trait;

use crate::OverlayError;

#[cfg(target_os = "linux")]
pub(crate) mod linux;

#[cfg(target_os = "macos")]
pub(crate) mod macos;

#[cfg(windows)]
pub(crate) mod windows;

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub(crate) mod stub;

/// Platform-agnostic operations for configuring a TUN interface.
///
/// Implementations live per-OS:
/// - Linux: [`linux::LinuxNetlinkOps`] (RTNETLINK via the `rtnetlink` crate)
/// - macOS: [`macos::MacIfconfigOps`] (shells to `ifconfig` / `route`)
/// - Windows: [`windows::WindowsIpHelperOps`] (IP Helper API via
///   `windows-rs`)
/// - Other: [`stub::StubOps`] (returns errors)
#[async_trait]
pub(crate) trait InterfaceOps: Send + Sync {
    /// Return true iff a network interface with this name exists on the host.
    ///
    /// Only wired to callers on Linux today; on Windows the Wintun adapter
    /// lifecycle is owned by the `tun::WindowsTun` session and callers do
    /// not currently probe for interface existence.
    #[allow(dead_code)]
    async fn link_exists(&self, name: &str) -> Result<bool, OverlayError>;

    /// Delete a network interface. Idempotent — `Ok(())` if absent.
    ///
    /// Only wired to callers on Linux today (see [`link_exists`]).
    #[allow(dead_code)]
    async fn delete_link(&self, name: &str) -> Result<(), OverlayError>;

    /// Bring a network interface administratively up.
    async fn set_link_up(&self, name: &str) -> Result<(), OverlayError>;

    /// Set the MTU on the interface.
    ///
    /// The overlay TUN device carries WireGuard-in-WireGuard traffic over
    /// a mesh, so its MTU must be tuned below the physical link MTU or
    /// oversized packets silently blackhole. The error is returned to the
    /// caller, which decides whether an MTU failure is fatal — a transport
    /// degraded by an un-tuned MTU is still preferable to a dead overlay.
    async fn set_mtu(&self, name: &str, mtu: u32) -> Result<(), OverlayError>;

    /// Assign an IP with prefix length to the interface.
    async fn add_address(
        &self,
        name: &str,
        addr: IpAddr,
        prefix_len: u8,
    ) -> Result<(), OverlayError>;

    /// Install a link-scope route to `dest/prefix_len` via this interface.
    async fn add_route_via_dev(
        &self,
        dest: IpAddr,
        prefix_len: u8,
        name: &str,
    ) -> Result<(), OverlayError>;
}

/// Construct the platform's [`InterfaceOps`] implementation.
pub(crate) fn platform_ops() -> Box<dyn InterfaceOps> {
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::LinuxNetlinkOps::new())
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacIfconfigOps::new())
    }
    #[cfg(windows)]
    {
        Box::new(windows::WindowsIpHelperOps::new())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        Box::new(stub::StubOps)
    }
}
