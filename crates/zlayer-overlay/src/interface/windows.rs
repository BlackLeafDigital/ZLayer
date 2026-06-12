//! Windows implementation of [`InterfaceOps`] via the IP Helper API.
//!
//! Uses `windows-rs 0.62`'s `Win32::NetworkManagement::IpHelper` module
//! to program the Wintun adapter — no shell-outs, no `PowerShell`. The
//! three calls that matter:
//!
//! | [`InterfaceOps`] method | IP Helper call |
//! |-|-|
//! | [`InterfaceOps::add_address`]      | `CreateUnicastIpAddressEntry` |
//! | [`InterfaceOps::add_route_via_dev`]| `CreateIpForwardEntry2`       |
//! | [`InterfaceOps::delete_link`]      | *(no-op — Wintun drops the adapter when the session is dropped)* |
//!
//! The IP Helper API is LUID-based, while [`InterfaceOps`] is
//! name-based, so every call starts with a
//! [`ConvertInterfaceAliasToLuid`] lookup.

// This module is the Windows IP Helper API FFI boundary. Every `unsafe`
// block below has a `SAFETY:` comment explaining why the required
// invariants hold; the workspace-wide `-W unsafe-code` policy remains in
// force everywhere else. (`#[cfg(windows)]` is applied by the parent
// `interface.rs` module declaration, so it is not repeated here.)
// `borrow_as_ptr` is allowed because the IP Helper entry points take
// `*mut T` / `*const T` and we hand them `&mut row` / `&row` at the call
// sites — each inside an `unsafe` block with a `SAFETY:` comment.
#![allow(unsafe_code, clippy::borrow_as_ptr)]

use std::net::IpAddr;

use async_trait::async_trait;
use tokio::task;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_OBJECT_ALREADY_EXISTS, NO_ERROR};
use windows::Win32::NetworkManagement::IpHelper::{
    ConvertInterfaceAliasToLuid, CreateIpForwardEntry2, CreateUnicastIpAddressEntry,
    InitializeIpForwardEntry, InitializeUnicastIpAddressEntry, MIB_IPFORWARD_ROW2,
    MIB_UNICASTIPADDRESS_ROW,
};
use windows::Win32::NetworkManagement::Ndis::NET_LUID_LH;
use windows::Win32::Networking::WinSock::{
    ADDRESS_FAMILY, AF_INET, AF_INET6, IN6_ADDR, IN6_ADDR_0, IN_ADDR, IN_ADDR_0, SOCKADDR_IN,
    SOCKADDR_IN6, SOCKADDR_IN6_0, SOCKADDR_INET,
};

use crate::interface::InterfaceOps;
use crate::OverlayError;

/// Protocol constant for a user-installed route. IP Helper's
/// `MIB_IPFORWARD_ROW2.Protocol` field takes a `NL_ROUTE_PROTOCOL` enum
/// whose `MIB_IPPROTO_NETMGMT` variant is 3 and means "manually
/// configured" — same value that `netsh interface ipv4 add route` uses.
const MIB_IPPROTO_NETMGMT: i32 = 3;

/// `InterfaceOps` implementation for Windows hosts.
pub(crate) struct WindowsIpHelperOps;

impl WindowsIpHelperOps {
    pub(crate) fn new() -> Self {
        Self
    }
}

#[async_trait]
impl InterfaceOps for WindowsIpHelperOps {
    async fn link_exists(&self, name: &str) -> Result<bool, OverlayError> {
        let owned = name.to_string();
        task::spawn_blocking(move || Ok(luid_for_name(&owned).is_ok()))
            .await
            .map_err(|e| OverlayError::NetworkConfig(format!("join error: {e}")))?
    }

    async fn delete_link(&self, _name: &str) -> Result<(), OverlayError> {
        // On Windows we do not delete the interface through IP Helper —
        // Wintun owns the adapter lifecycle and removes it when the
        // `WindowsTun` session is dropped. Calling this is a no-op so
        // the platform-agnostic transport code stays symmetric.
        Ok(())
    }

    async fn set_link_up(&self, _name: &str) -> Result<(), OverlayError> {
        // Wintun adapters are "up" as soon as the session is started.
        // `ifconfig up` has no equivalent — provide a no-op so
        // `OverlayTransport::configure_interface` keeps working.
        Ok(())
    }

    async fn set_mtu(&self, _name: &str, _mtu: u32) -> Result<(), OverlayError> {
        // Wintun exposes no per-adapter MTU setter via IP Helper; the MTU is fixed at adapter-create time, so this is a no-op.
        Ok(())
    }

    async fn add_address(
        &self,
        name: &str,
        addr: IpAddr,
        prefix_len: u8,
    ) -> Result<(), OverlayError> {
        let owned_name = name.to_string();
        task::spawn_blocking(move || add_address_blocking(&owned_name, addr, prefix_len))
            .await
            .map_err(|e| OverlayError::NetworkConfig(format!("join error: {e}")))?
    }

    async fn add_route_via_dev(
        &self,
        dest: IpAddr,
        prefix_len: u8,
        name: &str,
    ) -> Result<(), OverlayError> {
        let owned_name = name.to_string();
        task::spawn_blocking(move || add_route_blocking(&owned_name, dest, prefix_len))
            .await
            .map_err(|e| OverlayError::NetworkConfig(format!("join error: {e}")))?
    }
}

/// Convert an interface alias (the name the user sees in Device Manager
/// / netsh / `Get-NetAdapter`) to a `NET_LUID_LH`.
///
/// Returns [`OverlayError::InterfaceNotFound`] if the alias is unknown
/// on the host — callers rely on this distinction to preserve
/// idempotent "not there" semantics on `delete_link` / `link_exists`.
fn luid_for_name(name: &str) -> Result<NET_LUID_LH, OverlayError> {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut luid = NET_LUID_LH::default();
    // SAFETY: `wide` is a valid NUL-terminated UTF-16 buffer that
    // out-lives the call; `luid` is a valid mutable reference.
    let rc = unsafe { ConvertInterfaceAliasToLuid(PCWSTR::from_raw(wide.as_ptr()), &mut luid) };
    if rc == NO_ERROR {
        Ok(luid)
    } else {
        Err(OverlayError::InterfaceNotFound(name.to_string()))
    }
}

/// Construct a `SOCKADDR_INET` union from a Rust `IpAddr`.
///
/// `MIB_UNICASTIPADDRESS_ROW.Address` and
/// `MIB_IPFORWARD_ROW2.DestinationPrefix.Prefix` both expect this
/// shape; IP Helper reads the family out of the union and interprets
/// the appropriate arm.
fn sockaddr_inet_from(addr: IpAddr) -> SOCKADDR_INET {
    let mut sa = SOCKADDR_INET::default();
    match addr {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            // Pack the IPv4 octets into the union's native representation.
            let packed = u32::from_ne_bytes(octets);
            sa.Ipv4 = SOCKADDR_IN {
                sin_family: AF_INET,
                sin_port: 0,
                sin_addr: IN_ADDR {
                    S_un: IN_ADDR_0 { S_addr: packed },
                },
                sin_zero: [0; 8],
            };
        }
        IpAddr::V6(v6) => {
            sa.Ipv6 = SOCKADDR_IN6 {
                sin6_family: AF_INET6,
                sin6_port: 0,
                sin6_flowinfo: 0,
                sin6_addr: IN6_ADDR {
                    u: IN6_ADDR_0 { Byte: v6.octets() },
                },
                Anonymous: SOCKADDR_IN6_0 { sin6_scope_id: 0 },
            };
        }
    }
    sa
}

/// Family constant matching an `IpAddr`. Pushed into
/// `MIB_IPFORWARD_ROW2.DestinationPrefix.Prefix.si_family` / the
/// `ADDRESS_FAMILY` slots that IP Helper uses to discriminate the
/// `SOCKADDR_INET` union.
fn address_family(addr: IpAddr) -> ADDRESS_FAMILY {
    match addr {
        IpAddr::V4(_) => AF_INET,
        IpAddr::V6(_) => AF_INET6,
    }
}

/// Synchronous core of [`WindowsIpHelperOps::add_address`]. Runs inside
/// `spawn_blocking`.
fn add_address_blocking(name: &str, addr: IpAddr, prefix_len: u8) -> Result<(), OverlayError> {
    let luid = luid_for_name(name)?;

    let mut row = MIB_UNICASTIPADDRESS_ROW::default();
    // SAFETY: `row` is a valid mutable reference to a zeroed struct.
    unsafe { InitializeUnicastIpAddressEntry(&mut row) };
    row.InterfaceLuid = luid;
    row.Address = sockaddr_inet_from(addr);
    row.OnLinkPrefixLength = prefix_len;
    // `DadState = IpDadStatePreferred` skips duplicate-address detection,
    // which is appropriate for a point-to-point TUN device.
    // The default value from `InitializeUnicastIpAddressEntry` is already
    // `IpDadStatePreferred` (= 4), so we don't override it here.

    // SAFETY: `row` is fully initialized above; IP Helper reads from
    // the pointer synchronously and does not retain it.
    let rc = unsafe { CreateUnicastIpAddressEntry(&row) };
    if rc == NO_ERROR || rc == ERROR_OBJECT_ALREADY_EXISTS {
        Ok(())
    } else {
        Err(OverlayError::NetworkConfig(format!(
            "CreateUnicastIpAddressEntry failed for {addr}/{prefix_len} on {name}: WIN32 error {}",
            rc.0
        )))
    }
}

/// Synchronous core of [`WindowsIpHelperOps::add_route_via_dev`].
fn add_route_blocking(name: &str, dest: IpAddr, prefix_len: u8) -> Result<(), OverlayError> {
    let luid = luid_for_name(name)?;

    let mut row = MIB_IPFORWARD_ROW2::default();
    // SAFETY: `row` is a valid mutable reference to a zeroed struct.
    unsafe { InitializeIpForwardEntry(&mut row) };

    row.InterfaceLuid = luid;

    // Destination prefix.
    row.DestinationPrefix.Prefix = sockaddr_inet_from(dest);
    row.DestinationPrefix.PrefixLength = prefix_len;

    // Next hop: unspecified → the kernel treats it as on-link through
    // the interface identified by `InterfaceLuid`. This matches
    // `route add <net>/<pfx> 0.0.0.0 if <idx>` on Windows.
    row.NextHop = SOCKADDR_INET::default();
    // The family slot inside the SOCKADDR_INET union must still be set
    // so IP Helper knows which address family this row belongs to.
    row.NextHop.si_family = address_family(dest);

    // A modest metric keeps the route below the default gateway in
    // route resolution order when multiple adapters exist.
    row.Metric = 256;
    // MIB_IPPROTO_NETMGMT marks the route as user-installed so netsh /
    // Get-NetRoute renders it as such.
    row.Protocol.0 = MIB_IPPROTO_NETMGMT;

    // SAFETY: `row` is fully initialized above; IP Helper reads from
    // the pointer synchronously and does not retain it.
    let rc = unsafe { CreateIpForwardEntry2(&row) };
    if rc == NO_ERROR || rc == ERROR_OBJECT_ALREADY_EXISTS {
        Ok(())
    } else {
        Err(OverlayError::NetworkConfig(format!(
            "CreateIpForwardEntry2 failed for {dest}/{prefix_len} via {name}: WIN32 error {}",
            rc.0
        )))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn sockaddr_inet_round_trips_ipv4() {
        let sa = sockaddr_inet_from(IpAddr::V4(Ipv4Addr::new(10, 200, 0, 5)));
        // SAFETY: we just populated the Ipv4 arm above.
        let family = unsafe { sa.si_family };
        assert_eq!(family, AF_INET);
        // SAFETY: Ipv4 arm is active.
        let packed = unsafe { sa.Ipv4.sin_addr.S_un.S_addr };
        let octets = packed.to_ne_bytes();
        assert_eq!(octets, [10, 200, 0, 5]);
    }

    #[test]
    fn sockaddr_inet_round_trips_ipv6() {
        let ip = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1);
        let sa = sockaddr_inet_from(IpAddr::V6(ip));
        // SAFETY: we just populated the Ipv6 arm above.
        let family = unsafe { sa.si_family };
        assert_eq!(family, AF_INET6);
        // SAFETY: Ipv6 arm is active.
        let bytes = unsafe { sa.Ipv6.sin6_addr.u.Byte };
        assert_eq!(bytes, ip.octets());
    }

    #[test]
    fn address_family_matches() {
        assert_eq!(address_family(IpAddr::V4(Ipv4Addr::LOCALHOST)), AF_INET);
        assert_eq!(address_family(IpAddr::V6(Ipv6Addr::LOCALHOST)), AF_INET6);
    }

    #[tokio::test]
    #[ignore = "Requires Administrator + Wintun adapter"]
    async fn add_address_requires_existing_adapter() {
        let ops = WindowsIpHelperOps::new();
        let res = ops
            .add_address(
                "this-adapter-definitely-does-not-exist",
                IpAddr::V4(Ipv4Addr::new(10, 200, 0, 1)),
                24,
            )
            .await;
        // We expect a `InterfaceNotFound` from the LUID lookup — assert
        // it surfaces cleanly without panicking.
        assert!(res.is_err());
    }
}
