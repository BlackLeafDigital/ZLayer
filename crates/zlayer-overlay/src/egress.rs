//! Physical-egress resolver: find the real NIC the box uses to reach the
//! outside world, ignoring VPN-mesh interfaces.
//!
//! # Why this exists
//!
//! Everywhere in the overlay stack, "what is my source IP?" has historically
//! been answered with the UDP-connect trick: bind a UDP socket to `0.0.0.0:0`,
//! `connect()` it to a public address (`8.8.8.8:80`), and read `local_addr`.
//! That returns the source address the kernel would pick for the **default
//! route**. It works fine on a box with a single physical uplink.
//!
//! It breaks the moment a VPN mesh (netbird `wt0`, raw `WireGuard` `wg0`, `Tailscale`
//! `utun`/`tailscale0`, …) owns the default route — which is the normal state on
//! a mesh-joined node. The trick then reports the *mesh's* tunnel IP, and the
//! overlay ends up advertising and binding through the VPN instead of the real
//! NIC. Peers on other physical networks can't reach a mesh-internal address,
//! and we get a tunnel-inside-a-tunnel.
//!
//! [`detect_physical_egress`] resolves the **physical** egress interface and its
//! address, skipping virtual/mesh interfaces. On Linux it reads the routing
//! table directly via RTNETLINK (same connection pattern as
//! [`crate::netlink`]); on macOS it falls back to the UDP-connect trick plus a
//! `getifaddrs` interface-name lookup; on Windows it keeps the UDP-connect trick
//! verbatim (mesh coexistence on Windows is not handled yet).
//!
//! [`bind_to_device`] is the companion that pins a socket to a named interface
//! (`SO_BINDTODEVICE` on Linux, `IP_BOUND_IF` on macOS) so traffic actually
//! leaves through the resolved NIC.

use std::net::IpAddr;

use crate::error::OverlayError;

/// Interface-name prefixes (and exact names) that are *virtual* for the
/// purposes of physical-egress detection: loopback, VPN-mesh tunnels, our own
/// overlay devices, container bridges, and veth pairs. An interface whose name
/// matches any entry here must never be chosen as the physical egress.
///
/// Membership notes:
/// - `lo`               — loopback (matched as a prefix; `lo`, `lo0` both hit).
/// - `wt`               — netbird `WireGuard` tunnels (`wt0`, …).
/// - `wg`               — generic kernel/userspace `WireGuard` (`wg0`, …).
/// - `zl-`              — `ZLayer`'s own overlay TUN devices (see
///   `DEFAULT_INTERFACE_NAME` / `make_interface_name`).
/// - `utun`             — Apple userspace tunnels, also used by Tailscale/WG on
///   macOS (`utun0`, …).
/// - `tun` / `tap`      — generic L3 / L2 userspace tunnels.
/// - `docker` / `br-`   — Docker's default bridge and per-network bridges.
/// - `veth`             — container veth pairs.
/// - `nb`               — netbird's alternate interface naming (`nb-wt`, …).
const VIRTUAL_IFACE_PREFIXES: &[&str] = &[
    "lo", "wt", "wg", "zl-", "utun", "tun", "tap", "docker", "br-", "veth", "nb",
];

/// The physical interface the host uses to reach the outside world, and the
/// source IP bound to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhysicalEgress {
    /// Name of the physical egress interface (e.g. `eth0`, `enp3s0`, `en0`).
    ///
    /// Empty string on Windows, where only the IP is resolved (see
    /// [`detect_physical_egress`]).
    pub interface: String,
    /// Source IP address bound to [`PhysicalEgress::interface`].
    pub ip: IpAddr,
}

/// Returns `true` if `name` is a virtual / mesh / container interface that must
/// be skipped when choosing the physical egress.
///
/// Matching is by prefix against [`VIRTUAL_IFACE_PREFIXES`], so `wt0`, `wg0`,
/// `zl-abc`, `utun3`, `tun0`, `docker0`, `br-1`, `veth9a`, and `nb-wt` are all
/// virtual, while `eth0`, `en0`, `enp3s0`, `wlan0`, `wlp2s0`, and `bond0` are
/// not.
#[must_use]
pub fn is_virtual_interface(name: &str) -> bool {
    VIRTUAL_IFACE_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

/// Discover the local IPv4 address via the UDP-connect trick.
///
/// Binds a UDP socket and `connect()`s it to a public address; no packets are
/// sent (UDP `connect` only sets the default destination), so this just makes
/// the kernel pick the source address for the **default route**. This is the
/// last-resort fallback on Linux and the primary candidate on macOS/Windows.
fn udp_connect_local_ipv4() -> std::result::Result<IpAddr, std::io::Error> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
    socket.connect("8.8.8.8:80")?;
    Ok(socket.local_addr()?.ip())
}

/// Discover the local IPv6 address via the UDP-connect trick (v6 variant of
/// [`udp_connect_local_ipv4`], connecting to Google's public DNS over v6).
///
/// Only the Linux egress path (`mod linux`) consumes this; gate it exactly to
/// that consumer so non-Linux test builds don't flag it as dead code.
#[cfg(target_os = "linux")]
fn udp_connect_local_ipv6() -> std::result::Result<IpAddr, std::io::Error> {
    let socket = std::net::UdpSocket::bind("[::]:0")?;
    socket.connect("[2001:4860:4860::8888]:80")?;
    Ok(socket.local_addr()?.ip())
}

/// Resolve the physical egress interface (the real NIC used to reach the
/// outside world) and its source IP, ignoring VPN-mesh / virtual interfaces.
///
/// # Platform behaviour
///
/// - **Linux**: reads the routing table over RTNETLINK. Default routes are
///   examined in ascending-metric order; the first whose output interface is
///   *not* virtual ([`is_virtual_interface`]) wins, and that interface's
///   primary global-scope address is returned (IPv4 preferred). If every
///   default route is virtual, it enumerates interfaces and picks the first
///   non-virtual UP interface that has a global address. If even that fails it
///   falls back to the UDP-connect trick and logs a warning.
/// - **macOS**: uses the UDP-connect trick to get a candidate IP, maps it to an
///   interface via `getifaddrs`, and — if that interface is virtual — scans
///   `getifaddrs` for the first UP, non-virtual, non-loopback interface with a
///   global IPv4. The asymmetry with Linux is deliberate: macOS has no
///   RTNETLINK, so route-metric ordering is unavailable and we lean on the
///   kernel's own source-selection plus an interface-name filter.
/// - **Windows**: returns the UDP-connect result with an empty interface name.
///   Mesh coexistence on Windows is not handled yet; this is a real fallback,
///   not a stub.
///
/// # Errors
///
/// Returns [`OverlayError::NetworkConfig`] if no physical egress IP can be
/// determined at all (no default route *and* no usable interface *and* the
/// UDP-connect fallback fails — e.g. a fully offline host).
pub async fn detect_physical_egress() -> Result<PhysicalEgress, OverlayError> {
    #[cfg(target_os = "linux")]
    {
        linux::detect().await
    }

    #[cfg(target_os = "macos")]
    {
        macos::detect()
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        // Windows (and any other OS): keep the UDP-connect trick verbatim.
        // We cannot distinguish a mesh interface here, so we report the IP the
        // kernel would use for the default route and leave the interface name
        // empty for callers to interpret as "unknown / OS default".
        let ip = udp_connect_local_ipv4().map_err(|e| {
            OverlayError::NetworkConfig(format!(
                "failed to determine local egress IP via UDP-connect: {e}"
            ))
        })?;
        Ok(PhysicalEgress {
            interface: String::new(),
            ip,
        })
    }
}

/// Pin a socket so its traffic egresses through the named interface.
///
/// - **Linux**: `setsockopt(SO_BINDTODEVICE, ifname)`.
/// - **macOS**: `setsockopt(IP_BOUND_IF, if_nametoindex(ifname))`.
/// - **Windows / other**: no-op `Ok(())` (interface pinning is handled
///   elsewhere on Windows; see [`detect_physical_egress`]).
///
/// `socket` is any type exposing a raw socket descriptor (a
/// [`std::net::UdpSocket`], [`std::net::TcpStream`], etc. all qualify via
/// `AsRawFd`). The crate does not pull in `socket2` outside the `nat` feature,
/// so this goes straight to `libc::setsockopt` on the borrowed fd rather than
/// wrapping a `socket2::Socket`.
///
/// # Privilege caveat
///
/// `SO_BINDTODEVICE` on Linux requires `CAP_NET_RAW` (or root). The `ZLayer`
/// daemon runs as root on hub nodes, so this succeeds there; non-root callers
/// will get `EPERM`. Call sites are expected to degrade to a warning rather
/// than fail hard when this returns `PermissionDenied` (that wiring lives in
/// the caller, added separately).
///
/// # Errors
///
/// Returns [`OverlayError::PermissionDenied`] if the kernel rejects the bind
/// for lack of privilege (`EPERM`), [`OverlayError::InterfaceNotFound`] if the
/// named interface does not exist (`ENODEV` / unknown index), and
/// [`OverlayError::NetworkConfig`] for any other `setsockopt` failure.
///
/// # Platform note (cfg split)
///
/// This function is `cfg`-split into two implementations sharing the one public
/// name and call-site ergonomics:
///
/// - On **unix** (`#[cfg(unix)]`) it takes `S: std::os::fd::AsRawFd` — the
///   `std::os::fd` module exists only on unix — and performs the real
///   `setsockopt` (Linux `SO_BINDTODEVICE`, macOS `IP_BOUND_IF`), honouring the
///   `PermissionDenied`-degrade contract documented above. On a unix target that
///   is neither Linux nor macOS it is a no-op `Ok(())`.
/// - On **non-unix** (`#[cfg(not(unix))]`, e.g. Windows) it is a no-op `Ok(())`
///   with no `AsRawFd` bound, because `std::os::fd` does not exist there and
///   interface pinning is handled elsewhere (see [`detect_physical_egress`]).
#[cfg(unix)]
#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos")),
    allow(unused_variables)
)]
pub fn bind_to_device<S>(socket: &S, interface: &str) -> Result<(), OverlayError>
where
    S: std::os::fd::AsRawFd,
{
    #[cfg(target_os = "linux")]
    {
        linux::bind_to_device(socket.as_raw_fd(), interface)
    }

    #[cfg(target_os = "macos")]
    {
        macos::bind_to_device(socket.as_raw_fd(), interface)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        // A unix that is neither Linux nor macOS: no-op. (`as_raw_fd` is still
        // satisfiable here via the `AsRawFd` bound, but we have no portable
        // interface-pinning syscall to call.)
        Ok(())
    }
}

/// Non-unix (Windows and anything else) variant of [`bind_to_device`]: a no-op
/// `Ok(())`.
///
/// `std::os::fd` does not exist off unix, so this overload drops the `AsRawFd`
/// bound entirely and keeps the signature uniform for cross-platform callers
/// that gate on cfg. Interface pinning on Windows is handled elsewhere (see
/// [`detect_physical_egress`]).
#[cfg(not(unix))]
#[allow(clippy::missing_errors_doc)]
pub fn bind_to_device<S>(_socket: &S, _interface: &str) -> Result<(), OverlayError> {
    Ok(())
}

// ---------------------------------------------------------------------------
// Linux implementation (RTNETLINK + SO_BINDTODEVICE)
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod linux {
    // The only `unsafe` here is the single `libc::setsockopt` FFI call in
    // `bind_to_device`; every use is a documented C-ABI boundary, not a memory
    // trick. Scoped to this module so the rest of the crate keeps the
    // warn-level `unsafe_code` lint.
    #![allow(unsafe_code)]

    use std::net::IpAddr;

    use futures_util::stream::TryStreamExt;
    use netlink_packet_route::{
        address::{AddressAttribute, AddressScope},
        link::{LinkAttribute, LinkFlag},
        route::{RouteAttribute, RouteHeader},
    };
    use rtnetlink::{Handle, IpVersion};

    use super::{
        is_virtual_interface, udp_connect_local_ipv4, udp_connect_local_ipv6, PhysicalEgress,
    };
    use crate::error::OverlayError;

    /// A default route candidate extracted from the main routing table.
    struct DefaultRoute {
        oif: u32,
        metric: u32,
    }

    pub(super) async fn detect() -> Result<PhysicalEgress, OverlayError> {
        let (connection, handle, _) = rtnetlink::new_connection().map_err(|e| {
            OverlayError::NetworkConfig(format!("rtnetlink new_connection failed: {e}"))
        })?;
        tokio::spawn(connection);

        // 1. Collect default routes from the main table, v4 first then v6.
        let mut defaults = collect_default_routes(&handle, IpVersion::V4).await?;
        let v4_only_defaults = defaults.len();
        defaults.extend(collect_default_routes(&handle, IpVersion::V6).await?);

        // Ascending metric: cheapest route wins, exactly like the kernel.
        defaults.sort_by_key(|r| r.metric);

        // 2. Walk default routes; first non-virtual oif with a usable address wins.
        for route in &defaults {
            let Some(name) = link_name_for_index(&handle, route.oif).await? else {
                continue;
            };
            if is_virtual_interface(&name) {
                continue;
            }
            if let Some(ip) = primary_global_addr(&handle, route.oif).await? {
                return Ok(PhysicalEgress {
                    interface: name,
                    ip,
                });
            }
        }

        // 3. All default routes were virtual (or address-less). Box genuinely
        //    routes everything through a mesh. Enumerate interfaces and take the
        //    first non-virtual UP interface with a global address.
        if let Some(egress) = first_non_virtual_up_with_addr(&handle).await? {
            tracing::warn!(
                v4_default_routes = v4_only_defaults,
                total_default_routes = defaults.len(),
                "all default routes egress through virtual/mesh interfaces; \
                 selected first non-virtual UP interface '{}' as physical egress",
                egress.interface,
            );
            return Ok(egress);
        }

        // 4. Last resort: the UDP-connect trick. We could not distinguish a
        //    physical NIC, so warn loudly — the returned IP may be a mesh IP.
        let ip = udp_connect_local_ipv4()
            .or_else(|_| udp_connect_local_ipv6())
            .map_err(|e| {
                OverlayError::NetworkConfig(format!(
                    "no physical egress interface found and UDP-connect fallback failed: {e}"
                ))
            })?;
        tracing::warn!(
            resolved_ip = %ip,
            "could not distinguish a physical NIC from virtual/mesh interfaces; \
             falling back to UDP-connect source IP (may be a VPN-mesh address)"
        );
        Ok(PhysicalEgress {
            interface: String::new(),
            ip,
        })
    }

    /// Collect default routes (`destination_prefix_length == 0`) from the main
    /// routing table for the given IP version, with their output interface and
    /// metric.
    async fn collect_default_routes(
        handle: &Handle,
        version: IpVersion,
    ) -> Result<Vec<DefaultRoute>, OverlayError> {
        let mut routes = handle.route().get(version).execute();
        let mut out = Vec::new();

        while let Some(route) = routes
            .try_next()
            .await
            .map_err(|e| OverlayError::NetworkConfig(format!("route dump failed: {e}")))?
        {
            // Only the main table; ignore local/broadcast/per-VRF tables. The
            // table id can live either in the header byte or in an RTA_TABLE
            // attribute when it exceeds 255.
            let mut table = u32::from(route.header.table);
            let mut oif: Option<u32> = None;
            let mut metric: u32 = 0;

            if route.header.destination_prefix_length != 0 {
                continue;
            }

            for attr in &route.attributes {
                match attr {
                    RouteAttribute::Oif(idx) => oif = Some(*idx),
                    RouteAttribute::Priority(m) => metric = *m,
                    RouteAttribute::Table(t) => table = *t,
                    _ => {}
                }
            }

            if table != u32::from(RouteHeader::RT_TABLE_MAIN) {
                continue;
            }

            if let Some(oif) = oif {
                out.push(DefaultRoute { oif, metric });
            }
        }

        Ok(out)
    }

    /// Resolve an interface name from its index, or `None` if it has vanished.
    async fn link_name_for_index(
        handle: &Handle,
        index: u32,
    ) -> Result<Option<String>, OverlayError> {
        let mut links = handle.link().get().match_index(index).execute();
        let Some(link) = links.try_next().await.map_err(|e| {
            OverlayError::NetworkConfig(format!("link lookup for index {index} failed: {e}"))
        })?
        else {
            return Ok(None);
        };

        Ok(link.attributes.iter().find_map(|attr| match attr {
            LinkAttribute::IfName(name) => Some(name.clone()),
            _ => None,
        }))
    }

    /// Read the primary global-scope address of an interface by index,
    /// preferring IPv4 over IPv6.
    async fn primary_global_addr(
        handle: &Handle,
        index: u32,
    ) -> Result<Option<IpAddr>, OverlayError> {
        let mut addrs = handle
            .address()
            .get()
            .set_link_index_filter(index)
            .execute();

        let mut v6: Option<IpAddr> = None;

        while let Some(msg) = addrs.try_next().await.map_err(|e| {
            OverlayError::NetworkConfig(format!("address dump for index {index} failed: {e}"))
        })? {
            // Only globally-scoped addresses are usable as an egress source.
            if msg.header.scope != AddressScope::Universe {
                continue;
            }
            for attr in &msg.attributes {
                if let AddressAttribute::Address(ip) = attr {
                    match ip {
                        IpAddr::V4(_) => return Ok(Some(*ip)), // v4 preferred
                        IpAddr::V6(_) => {
                            if v6.is_none() {
                                v6 = Some(*ip);
                            }
                        }
                    }
                }
            }
        }

        Ok(v6)
    }

    /// Enumerate links and return the first non-virtual, UP interface that has a
    /// global address.
    async fn first_non_virtual_up_with_addr(
        handle: &Handle,
    ) -> Result<Option<PhysicalEgress>, OverlayError> {
        let mut links = handle.link().get().execute();

        // Gather (index, name) for UP, non-virtual links first, then resolve an
        // address for each in order. Collected up-front because the address
        // dump borrows the same handle.
        let mut candidates: Vec<(u32, String)> = Vec::new();

        while let Some(link) = links
            .try_next()
            .await
            .map_err(|e| OverlayError::NetworkConfig(format!("link dump failed: {e}")))?
        {
            let is_up = link.header.flags.contains(&LinkFlag::Up);
            if !is_up {
                continue;
            }
            let name = link.attributes.iter().find_map(|attr| match attr {
                LinkAttribute::IfName(n) => Some(n.clone()),
                _ => None,
            });
            let Some(name) = name else { continue };
            if is_virtual_interface(&name) {
                continue;
            }
            candidates.push((link.header.index, name));
        }

        for (index, name) in candidates {
            if let Some(ip) = primary_global_addr(handle, index).await? {
                return Ok(Some(PhysicalEgress {
                    interface: name,
                    ip,
                }));
            }
        }

        Ok(None)
    }

    /// `setsockopt(SO_BINDTODEVICE)` on a raw fd.
    pub(super) fn bind_to_device(fd: i32, interface: &str) -> Result<(), OverlayError> {
        // SO_BINDTODEVICE takes the interface name (NUL-terminated) as the
        // option value, with length = strlen (kernel tolerates with/without NUL).
        let name_bytes = interface.as_bytes();
        // Guard against absurd names; IFNAMSIZ is 16 incl. NUL.
        if name_bytes.len() >= libc::IFNAMSIZ {
            return Err(OverlayError::NetworkConfig(format!(
                "interface name '{interface}' too long for SO_BINDTODEVICE (max {})",
                libc::IFNAMSIZ - 1
            )));
        }

        // SAFETY: `fd` is a valid socket fd for the lifetime of the borrowed
        // socket the caller holds; we pass a pointer/len pair describing the
        // interface-name byte slice, which `setsockopt` copies. No ownership of
        // the fd is taken.
        let rc = unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_BINDTODEVICE,
                name_bytes.as_ptr().cast::<libc::c_void>(),
                // Length already bounded < IFNAMSIZ above, so the conversion
                // never overflows; the `unwrap_or` is purely defensive.
                libc::socklen_t::try_from(name_bytes.len()).unwrap_or(0),
            )
        };

        if rc == 0 {
            return Ok(());
        }

        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::EPERM | libc::EACCES) => Err(OverlayError::PermissionDenied(format!(
                "SO_BINDTODEVICE({interface}) requires CAP_NET_RAW or root: {err}"
            ))),
            Some(libc::ENODEV) => Err(OverlayError::InterfaceNotFound(interface.to_string())),
            _ => Err(OverlayError::NetworkConfig(format!(
                "SO_BINDTODEVICE({interface}) failed: {err}"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// macOS implementation (getifaddrs + IP_BOUND_IF)
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod macos {
    // `unsafe` here is confined to the `getifaddrs`/`freeifaddrs`/`setsockopt`/
    // `if_nametoindex` FFI calls and the `sockaddr` re-interpretation needed to
    // read interface addresses — all documented C-ABI boundaries. Scoped to this
    // module so the rest of the crate keeps the warn-level `unsafe_code` lint.
    #![allow(unsafe_code)]

    use std::ffi::{CStr, CString};
    use std::net::{IpAddr, Ipv4Addr};

    use super::{is_virtual_interface, udp_connect_local_ipv4, PhysicalEgress};
    use crate::error::OverlayError;

    /// One interface entry distilled from `getifaddrs`.
    struct IfEntry {
        name: String,
        ip: IpAddr,
        is_up: bool,
        is_loopback: bool,
    }

    pub(super) fn detect() -> Result<PhysicalEgress, OverlayError> {
        // Candidate IP from the kernel's own source selection for the default route.
        let candidate = udp_connect_local_ipv4().map_err(|e| {
            OverlayError::NetworkConfig(format!(
                "failed to determine local egress IP via UDP-connect: {e}"
            ))
        })?;

        let entries = getifaddrs_entries()?;

        // Map candidate IP -> interface name.
        let candidate_iface = entries
            .iter()
            .find(|e| e.ip == candidate)
            .map(|e| e.name.clone());

        // If the candidate maps to a real (non-virtual) interface, use it as-is.
        if let Some(name) = candidate_iface {
            if !is_virtual_interface(&name) {
                return Ok(PhysicalEgress {
                    interface: name,
                    ip: candidate,
                });
            }
        }

        // Candidate was virtual (mesh owns the default route) or unmapped: scan
        // for the first UP, non-virtual, non-loopback interface with a global v4.
        for entry in &entries {
            if !entry.is_up || entry.is_loopback {
                continue;
            }
            if is_virtual_interface(&entry.name) {
                continue;
            }
            if let IpAddr::V4(v4) = entry.ip {
                if is_global_ipv4(v4) {
                    return Ok(PhysicalEgress {
                        interface: entry.name.clone(),
                        ip: entry.ip,
                    });
                }
            }
        }

        // Nothing better than the kernel's pick — return it (interface name may
        // be a mesh device). Warn so operators can see why.
        tracing::warn!(
            resolved_ip = %candidate,
            "macOS: could not distinguish a physical NIC; returning UDP-connect source IP"
        );
        Ok(PhysicalEgress {
            interface: String::new(),
            ip: candidate,
        })
    }

    /// IPv4 is "global" if it is not loopback, link-local (169.254/16),
    /// unspecified, or broadcast — i.e. a usable egress source.
    fn is_global_ipv4(v4: Ipv4Addr) -> bool {
        !v4.is_loopback()
            && !v4.is_link_local()
            && !v4.is_unspecified()
            && !v4.is_broadcast()
            && !v4.is_multicast()
    }

    /// Walk `getifaddrs` and distill IPv4/IPv6 address entries.
    fn getifaddrs_entries() -> Result<Vec<IfEntry>, OverlayError> {
        let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
        // SAFETY: `getifaddrs` writes a heap-allocated linked list into `head`
        // on success; we free it with `freeifaddrs` before returning.
        let rc = unsafe { libc::getifaddrs(&raw mut head) };
        if rc != 0 {
            return Err(OverlayError::NetworkConfig(format!(
                "getifaddrs failed: {}",
                std::io::Error::last_os_error()
            )));
        }

        let mut entries = Vec::new();
        let mut cur = head;
        // SAFETY: iterate the NUL-terminated linked list; each node and its
        // `ifa_name` / `ifa_addr` pointers are valid until `freeifaddrs`.
        while !cur.is_null() {
            let ifa = unsafe { &*cur };
            cur = ifa.ifa_next;

            if ifa.ifa_name.is_null() || ifa.ifa_addr.is_null() {
                continue;
            }

            let name = unsafe { CStr::from_ptr(ifa.ifa_name) }
                .to_string_lossy()
                .into_owned();

            let is_up = (ifa.ifa_flags & (libc::IFF_UP as u32)) != 0;
            let is_loopback = (ifa.ifa_flags & (libc::IFF_LOOPBACK as u32)) != 0;

            // SAFETY: `ifa_addr` points at a `sockaddr`; we read `sa_family`
            // then re-interpret as the matching `sockaddr_in`/`sockaddr_in6`.
            let family = i32::from(unsafe { (*ifa.ifa_addr).sa_family });
            let ip = match family {
                libc::AF_INET => {
                    // `read_unaligned` copies the value out by-bytes, so it makes
                    // no alignment assumption about the kernel's sockaddr storage.
                    let sin = unsafe {
                        std::ptr::read_unaligned(ifa.ifa_addr.cast::<libc::sockaddr_in>())
                    };
                    let raw = u32::from_be(sin.sin_addr.s_addr);
                    Some(IpAddr::V4(Ipv4Addr::from(raw)))
                }
                libc::AF_INET6 => {
                    let sin6 = unsafe {
                        std::ptr::read_unaligned(ifa.ifa_addr.cast::<libc::sockaddr_in6>())
                    };
                    Some(IpAddr::V6(std::net::Ipv6Addr::from(sin6.sin6_addr.s6_addr)))
                }
                _ => None,
            };

            if let Some(ip) = ip {
                entries.push(IfEntry {
                    name,
                    ip,
                    is_up,
                    is_loopback,
                });
            }
        }

        // SAFETY: `head` is the list returned by `getifaddrs`; freed exactly once.
        unsafe { libc::freeifaddrs(head) };

        Ok(entries)
    }

    /// `setsockopt(IP_BOUND_IF)` keyed by the interface's scope index.
    pub(super) fn bind_to_device(fd: i32, interface: &str) -> Result<(), OverlayError> {
        let cname = CString::new(interface).map_err(|_| {
            OverlayError::NetworkConfig(format!(
                "interface name '{interface}' contains an interior NUL byte"
            ))
        })?;

        // SAFETY: `cname` is a valid NUL-terminated C string for the call.
        let idx = unsafe { libc::if_nametoindex(cname.as_ptr()) };
        if idx == 0 {
            return Err(OverlayError::InterfaceNotFound(interface.to_string()));
        }

        let idx = libc::c_int::try_from(idx)
            .map_err(|_| OverlayError::InterfaceNotFound(interface.to_string()))?;
        // SAFETY: `fd` is a valid socket fd; we pass a pointer/len pair to a
        // local `c_int` holding the interface scope index, which the kernel copies.
        let rc = unsafe {
            libc::setsockopt(
                fd,
                libc::IPPROTO_IP,
                libc::IP_BOUND_IF,
                std::ptr::addr_of!(idx).cast::<libc::c_void>(),
                // `size_of::<c_int>()` is 4 on every supported target and always
                // fits in `socklen_t` (u32); the conversion cannot truncate.
                #[allow(clippy::cast_possible_truncation)]
                {
                    std::mem::size_of::<libc::c_int>() as libc::socklen_t
                },
            )
        };

        if rc == 0 {
            return Ok(());
        }

        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::EPERM | libc::EACCES) => Err(OverlayError::PermissionDenied(format!(
                "IP_BOUND_IF({interface}) denied: {err}"
            ))),
            _ => Err(OverlayError::NetworkConfig(format!(
                "IP_BOUND_IF({interface}) failed: {err}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_interfaces_are_detected() {
        for name in [
            "lo",
            "lo0",
            "wt0",
            "wg0",
            "zl-abc",
            "zl-overlay0",
            "utun3",
            "tun0",
            "tap0",
            "docker0",
            "br-1",
            "veth9a2b",
            "nb-wt",
        ] {
            assert!(
                is_virtual_interface(name),
                "expected '{name}' to be virtual"
            );
        }
    }

    #[test]
    fn physical_interfaces_are_not_virtual() {
        for name in ["eth0", "en0", "enp3s0", "wlan0", "wlp2s0", "bond0"] {
            assert!(
                !is_virtual_interface(name),
                "expected '{name}' to be physical (non-virtual)"
            );
        }
    }

    #[test]
    fn empty_name_is_not_virtual() {
        // Defensive: an empty interface name must not falsely match any prefix.
        assert!(!is_virtual_interface(""));
    }

    /// On a Linux dev box this should resolve a real egress. We accept either a
    /// successful detection (returns a non-loopback IP) or — in CI/network
    /// namespaces with no default route and no usable NIC — a graceful skip.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn detect_physical_egress_on_linux() {
        match detect_physical_egress().await {
            Ok(egress) => {
                // Whatever we resolved must not be loopback, and if an interface
                // name came back it must not be a virtual/mesh device.
                assert!(
                    !egress.ip.is_loopback(),
                    "resolved egress IP should not be loopback: {egress:?}"
                );
                if !egress.interface.is_empty() {
                    assert!(
                        !is_virtual_interface(&egress.interface),
                        "resolved egress interface should be physical: {egress:?}"
                    );
                }
            }
            Err(e) => {
                // No default route / no usable NIC (e.g. an isolated netns).
                // Skip gracefully rather than fail the suite.
                eprintln!("skipping: no physical egress detectable in this environment: {e}");
            }
        }
    }
}
