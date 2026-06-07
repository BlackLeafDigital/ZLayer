//! In-guest `WireGuard` overlay bring-up for the macOS VZ-Linux guest agent.
//!
//! A VM has no host-visible network namespace or PID, so the host cannot attach
//! an overlay `veth` into the guest the way it does for native Linux containers.
//! Instead the host's overlay daemon allocates this container's overlay identity
//! (keypair + address + the current peer set) and ships it over vsock as
//! [`Msg::OverlayConfig`](crate::proto::Msg::OverlayConfig); this module applies
//! it by standing up a **kernel** `WireGuard` device (`zl-overlay0`) entirely from
//! inside the guest.
//!
//! The kernel is built with `CONFIG_WIREGUARD=y` + `CONFIG_NETDEVICES=y`, so the
//! work splits across two netlink families:
//!
//! * **rtnetlink** (`NETLINK_ROUTE`) — create the `wireguard`-type link
//!   (`RTM_NEWLINK` + `IFLA_INFO_KIND="wireguard"`), assign the overlay address
//!   (`RTM_NEWADDR`), bring the link up (`RTM_NEWLINK` with `IFF_UP`), and add a
//!   route per peer `allowed_ips` CIDR (`RTM_NEWROUTE`).
//! * **generic netlink** (`NETLINK_GENERIC`) — resolve the dynamic `wireguard`
//!   genl family id via `nlctrl`, then `WG_CMD_SET_DEVICE` to install the private
//!   key, listen port, and every peer (public key, endpoint, allowed-ips,
//!   persistent-keepalive).
//!
//! Everything here is synchronous (no async runtime) and pure Rust, matching the
//! agent's runtime model and keeping the static-musl binary small. A failure to
//! bring up the overlay is logged and returned to the caller but never crashes
//! PID 1 — the workload may not need the overlay at all (mirroring how
//! `bring_up_network` tolerates failure).

use std::net::{IpAddr, SocketAddr};

use netlink_packet_core::{
    NetlinkMessage, NetlinkPayload, NLM_F_ACK, NLM_F_CREATE, NLM_F_EXCL, NLM_F_REQUEST,
};
use netlink_packet_generic::{
    ctrl::{nlas::GenlCtrlAttrs, GenlCtrl, GenlCtrlCmd},
    GenlMessage,
};
use netlink_packet_route::{
    address::{AddressAttribute, AddressMessage},
    link::{InfoKind, LinkAttribute, LinkFlags, LinkInfo, LinkMessage},
    route::{RouteAddress, RouteAttribute, RouteMessage, RouteProtocol, RouteScope, RouteType},
    AddressFamily, RouteNetlinkMessage,
};
use netlink_packet_wireguard::{
    WireguardAllowedIp, WireguardAllowedIpAttr, WireguardAttribute, WireguardCmd, WireguardMessage,
    WireguardPeer, WireguardPeerAttribute,
};
use netlink_sys::{protocols, Socket, SocketAddr as NlSocketAddr};

use zlayer_vzagent::proto::WgPeer;

use super::{err, Error, Result};

/// Name of the in-guest kernel `WireGuard` overlay interface.
pub const OVERLAY_IFNAME: &str = "zl-overlay0";

/// `WireGuard` key length (x25519 public/private key).
const WG_KEY_LEN: usize = 32;

/// Apply a full [`Msg::OverlayConfig`](crate::proto::Msg::OverlayConfig) inside
/// the guest: create + configure + bring up `zl-overlay0`, route the peers'
/// allowed-IPs through it, and install overlay DNS if provided.
///
/// Returns the first error encountered. The caller treats overlay failure as
/// non-fatal (logs it, keeps PID 1 running): a route/addr failure still leaves a
/// usable device for diagnostics, and a workload that does not use the overlay is
/// unaffected.
#[allow(clippy::too_many_arguments)]
pub fn apply_overlay_config(
    overlay_ip: &str,
    prefix_len: u8,
    private_key: &str,
    listen_port: u16,
    peers: &[WgPeer],
    dns_server: Option<&str>,
    dns_domain: Option<&str>,
) -> Result<()> {
    // 1. Create the kernel WireGuard netdev (rtnetlink RTM_NEWLINK, type
    //    "wireguard"). Tolerate EEXIST so a re-pushed OverlayConfig is
    //    idempotent (the host may resend the current peer set).
    create_wireguard_link(OVERLAY_IFNAME)?;

    // Resolve the interface index now that the link exists; addr/route/up all
    // address the device by index.
    let ifindex = if_nametoindex(OVERLAY_IFNAME)?;

    // 2. Configure crypto + peers via the WireGuard generic-netlink family
    //    (WG_CMD_SET_DEVICE): private key, listen port, and every peer.
    set_wireguard_device(OVERLAY_IFNAME, private_key, listen_port, peers)?;

    // 3. Assign the overlay address (RTM_NEWADDR) and bring the link UP
    //    (RTM_NEWLINK + IFF_UP).
    let addr: IpAddr = overlay_ip
        .parse()
        .map_err(|e| err(format!("overlay_ip {overlay_ip:?} parse: {e}")))?;
    add_interface_address(ifindex, addr, prefix_len)?;
    set_link_up_by_index(ifindex)?;

    // 4. Route each peer's allowed_ips CIDRs out of the overlay device so
    //    overlay-destined traffic egresses zl-overlay0. We deliberately install
    //    only the explicit allowed_ips CIDRs (never a default/0.0.0.0-0 route),
    //    so the overlay never hijacks the guest's NAT default route.
    for peer in peers {
        for cidr in peer.allowed_ips.split(',') {
            let cidr = cidr.trim();
            if cidr.is_empty() {
                continue;
            }
            let (dst, dst_prefix) = match parse_cidr(cidr) {
                Ok(parsed) => parsed,
                Err(e) => {
                    eprintln!("zlayer-vzagent: overlay: skipping bad allowed_ip {cidr:?}: {e}");
                    continue;
                }
            };
            if let Err(e) = add_route_via_device(ifindex, dst, dst_prefix) {
                // A single bad/duplicate route must not abort the rest of the
                // peer set; log and continue.
                eprintln!("zlayer-vzagent: overlay: route {cidr} via {OVERLAY_IFNAME} failed: {e}");
            }
        }
    }

    // 5. Install overlay DNS for the workload, if the host supplied it. The
    //    agent has already pivoted into the container root, so /etc/resolv.conf
    //    here is the file the workload will see.
    if dns_server.is_some() || dns_domain.is_some() {
        if let Err(e) = write_resolv_conf(dns_server, dns_domain) {
            eprintln!("zlayer-vzagent: overlay: writing /etc/resolv.conf failed: {e}");
        }
    }

    Ok(())
}

// ----------------------------------------------------------------------
// rtnetlink helpers (NETLINK_ROUTE)
// ----------------------------------------------------------------------

/// Open a connected `NETLINK_ROUTE` socket bound to an auto-assigned port.
fn open_route_socket() -> Result<Socket> {
    let mut socket = Socket::new(protocols::NETLINK_ROUTE)
        .map_err(|e| err(format!("netlink route socket: {e}")))?;
    socket
        .bind_auto()
        .map_err(|e| err(format!("netlink route bind: {e}")))?;
    socket
        .connect(&NlSocketAddr::new(0, 0))
        .map_err(|e| err(format!("netlink route connect: {e}")))?;
    Ok(socket)
}

/// Create the `wireguard`-type link named `ifname` via `RTM_NEWLINK`.
///
/// `EEXIST` (the device already exists from a prior `OverlayConfig`) is treated as
/// success so the operation is idempotent.
fn create_wireguard_link(ifname: &str) -> Result<()> {
    let mut link = LinkMessage::default();
    link.attributes
        .push(LinkAttribute::IfName(ifname.to_string()));
    link.attributes
        .push(LinkAttribute::LinkInfo(vec![LinkInfo::Kind(
            InfoKind::Wireguard,
        )]));

    let mut msg = NetlinkMessage::from(RouteNetlinkMessage::NewLink(link));
    msg.header.flags = NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_EXCL;
    match send_route_request(msg) {
        Ok(()) => Ok(()),
        // EEXIST: the device is already present; that's fine (idempotent).
        Err(NetlinkAckError::Kernel(code)) if code == -libc::EEXIST => Ok(()),
        Err(e) => Err(err(format!("create link {ifname}: {e}"))),
    }
}

/// Assign `addr/prefix_len` to the interface `ifindex` via `RTM_NEWADDR`.
fn add_interface_address(ifindex: u32, addr: IpAddr, prefix_len: u8) -> Result<()> {
    let mut amsg = AddressMessage::default();
    amsg.header.family = match addr {
        IpAddr::V4(_) => AddressFamily::Inet,
        IpAddr::V6(_) => AddressFamily::Inet6,
    };
    amsg.header.prefix_len = prefix_len;
    amsg.header.index = ifindex;
    // IFA_LOCAL + IFA_ADDRESS both carry the unicast address for a normal
    // (non-peer) interface, which is what `ip addr add <ip>/<n> dev ...` emits.
    amsg.attributes.push(AddressAttribute::Local(addr));
    amsg.attributes.push(AddressAttribute::Address(addr));

    let mut msg = NetlinkMessage::from(RouteNetlinkMessage::NewAddress(amsg));
    msg.header.flags = NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_EXCL;
    match send_route_request(msg) {
        Ok(()) => Ok(()),
        // Re-applying the same address is not an error.
        Err(NetlinkAckError::Kernel(code)) if code == -libc::EEXIST => Ok(()),
        Err(e) => Err(err(format!("add address {addr}/{prefix_len}: {e}"))),
    }
}

/// Bring the interface `ifindex` administratively up via `RTM_NEWLINK` with the
/// `IFF_UP` flag set (and the matching change mask).
fn set_link_up_by_index(ifindex: u32) -> Result<()> {
    let mut link = LinkMessage::default();
    link.header.index = ifindex;
    link.header.flags = LinkFlags::Up;
    link.header.change_mask = LinkFlags::Up;

    let mut msg = NetlinkMessage::from(RouteNetlinkMessage::SetLink(link));
    msg.header.flags = NLM_F_REQUEST | NLM_F_ACK;
    send_route_request(msg).map_err(|e| err(format!("set link up (ifindex {ifindex}): {e}")))
}

/// Add a route to `dst/dst_prefix` out of the device `ifindex` via
/// `RTM_NEWROUTE`. Scope is link-local (on-link, no gateway): overlay peers are
/// reachable directly through the `WireGuard` tunnel.
fn add_route_via_device(ifindex: u32, dst: IpAddr, dst_prefix: u8) -> Result<()> {
    let mut rmsg = RouteMessage::default();
    rmsg.header.address_family = match dst {
        IpAddr::V4(_) => AddressFamily::Inet,
        IpAddr::V6(_) => AddressFamily::Inet6,
    };
    rmsg.header.destination_prefix_length = dst_prefix;
    rmsg.header.table = libc::RT_TABLE_MAIN;
    rmsg.header.protocol = RouteProtocol::Boot;
    rmsg.header.scope = RouteScope::Link;
    rmsg.header.kind = RouteType::Unicast;
    rmsg.attributes
        .push(RouteAttribute::Destination(route_address(dst)));
    rmsg.attributes.push(RouteAttribute::Oif(ifindex));

    let mut msg = NetlinkMessage::from(RouteNetlinkMessage::NewRoute(rmsg));
    msg.header.flags = NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_EXCL;
    match send_route_request(msg) {
        Ok(()) => Ok(()),
        // A duplicate route (e.g. the on-link prefix already added by the addr,
        // or a re-pushed OverlayConfig) is not an error.
        Err(NetlinkAckError::Kernel(code)) if code == -libc::EEXIST => Ok(()),
        Err(e) => Err(err(format!("add route {dst}/{dst_prefix}: {e}"))),
    }
}

/// Convert an [`IpAddr`] into the route-crate's [`RouteAddress`] destination.
fn route_address(addr: IpAddr) -> RouteAddress {
    match addr {
        IpAddr::V4(v4) => RouteAddress::Inet(v4),
        IpAddr::V6(v6) => RouteAddress::Inet6(v6),
    }
}

// ----------------------------------------------------------------------
// WireGuard genetlink helpers (NETLINK_GENERIC)
// ----------------------------------------------------------------------

/// Open a connected `NETLINK_GENERIC` socket bound to an auto-assigned port.
fn open_genl_socket() -> Result<Socket> {
    let mut socket = Socket::new(protocols::NETLINK_GENERIC)
        .map_err(|e| err(format!("netlink generic socket: {e}")))?;
    socket
        .bind_auto()
        .map_err(|e| err(format!("netlink generic bind: {e}")))?;
    socket
        .connect(&NlSocketAddr::new(0, 0))
        .map_err(|e| err(format!("netlink generic connect: {e}")))?;
    Ok(socket)
}

/// Resolve the dynamically-assigned generic-netlink family id for the
/// `"wireguard"` family by asking `nlctrl` (`CTRL_CMD_GETFAMILY`).
fn resolve_wireguard_family_id(socket: &Socket) -> Result<u16> {
    let mut genl = GenlMessage::from_payload(GenlCtrl {
        cmd: GenlCtrlCmd::GetFamily,
        // The wireguard genl family name (matches WireguardMessage::family_name()).
        nlas: vec![GenlCtrlAttrs::FamilyName("wireguard".to_string())],
    });
    genl.finalize();
    let mut msg = NetlinkMessage::from(genl);
    msg.header.flags = NLM_F_REQUEST | NLM_F_ACK;
    msg.finalize();

    let mut buf = vec![0u8; msg.buffer_len()];
    msg.serialize(&mut buf);
    socket
        .send(&buf, 0)
        .map_err(|e| err(format!("genl GETFAMILY send: {e}")))?;

    // Read replies until we see the family id, a DONE, or an ERROR. A single
    // recv may carry multiple netlink messages; walk them by their declared
    // length.
    loop {
        let (reply_bytes, _) = socket
            .recv_from_full()
            .map_err(|e| err(format!("genl GETFAMILY recv: {e}")))?;
        let mut offset = 0;
        while offset < reply_bytes.len() {
            let reply =
                NetlinkMessage::<GenlMessage<GenlCtrl>>::deserialize(&reply_bytes[offset..])
                    .map_err(|e| err(format!("genl GETFAMILY decode: {e}")))?;
            let len = reply.header.length as usize;
            match reply.payload {
                NetlinkPayload::InnerMessage(genl) => {
                    for nla in genl.payload.nlas {
                        if let GenlCtrlAttrs::FamilyId(id) = nla {
                            return Ok(id);
                        }
                    }
                }
                NetlinkPayload::Error(e) => {
                    if let Some(code) = e.code {
                        return Err(err(format!(
                            "genl GETFAMILY(wireguard): kernel error {code}"
                        )));
                    }
                }
                NetlinkPayload::Done(_) => {
                    return Err(err("genl GETFAMILY(wireguard): no family id in reply"));
                }
                _ => {}
            }
            if len == 0 {
                break;
            }
            offset += len;
        }
    }
}

/// Configure the `WireGuard` device `ifname` over generic netlink
/// (`WG_CMD_SET_DEVICE`): set the private key + listen port and add every peer.
fn set_wireguard_device(
    ifname: &str,
    private_key: &str,
    listen_port: u16,
    peers: &[WgPeer],
) -> Result<()> {
    let socket = open_genl_socket()?;
    let family_id = resolve_wireguard_family_id(&socket)?;

    let priv_key = decode_wg_key(private_key).map_err(|e| err(format!("private_key: {e}")))?;

    let mut attributes = vec![
        WireguardAttribute::IfName(ifname.to_string()),
        WireguardAttribute::PrivateKey(priv_key),
        WireguardAttribute::ListenPort(listen_port),
    ];

    let mut wg_peers = Vec::with_capacity(peers.len());
    for peer in peers {
        wg_peers.push(build_wg_peer(peer)?);
    }
    if !wg_peers.is_empty() {
        attributes.push(WireguardAttribute::Peers(wg_peers));
    }

    let mut genl = GenlMessage::from_payload(WireguardMessage {
        cmd: WireguardCmd::SetDevice,
        attributes,
    });
    // The wireguard family id is dynamic; stamp it so message_type is correct.
    genl.set_resolved_family_id(family_id);
    genl.finalize();

    let mut msg = NetlinkMessage::from(genl);
    msg.header.flags = NLM_F_REQUEST | NLM_F_ACK;
    msg.finalize();

    let mut buf = vec![0u8; msg.buffer_len()];
    msg.serialize(&mut buf);
    socket
        .send(&buf, 0)
        .map_err(|e| err(format!("WG_CMD_SET_DEVICE send: {e}")))?;

    // Await the ACK / surface a kernel error.
    let (reply_bytes, _) = socket
        .recv_from_full()
        .map_err(|e| err(format!("WG_CMD_SET_DEVICE recv: {e}")))?;
    let reply = NetlinkMessage::<GenlMessage<WireguardMessage>>::deserialize(&reply_bytes)
        .map_err(|e| err(format!("WG_CMD_SET_DEVICE decode: {e}")))?;
    match reply.payload {
        // `NLMSG_ERROR` with code == None is a bare ACK (success). A non-None
        // code is always a negative errno from the kernel.
        NetlinkPayload::Error(e) => match e.code {
            None => Ok(()),
            Some(code) => Err(err(format!("WG_CMD_SET_DEVICE: kernel error {code}"))),
        },
        _ => Ok(()),
    }
}

/// Build a single [`WireguardPeer`] (the genl nested attribute set) from a
/// host-supplied [`WgPeer`].
fn build_wg_peer(peer: &WgPeer) -> Result<WireguardPeer> {
    let pub_key =
        decode_wg_key(&peer.public_key).map_err(|e| err(format!("peer public_key: {e}")))?;

    let mut attrs = vec![WireguardPeerAttribute::PublicKey(pub_key)];

    // Endpoint: empty string means a roaming peer (no fixed endpoint).
    let endpoint = peer.endpoint.trim();
    if !endpoint.is_empty() {
        let sa: SocketAddr = endpoint
            .parse()
            .map_err(|e| err(format!("peer endpoint {endpoint:?}: {e}")))?;
        attrs.push(WireguardPeerAttribute::Endpoint(sa));
    }

    // Persistent keepalive: 0 disables it (and is the kernel default), so only
    // emit the attribute when a non-zero interval was requested.
    if peer.persistent_keepalive_secs != 0 {
        let secs = u16::try_from(peer.persistent_keepalive_secs)
            .map_err(|_| err("peer keepalive exceeds u16"))?;
        attrs.push(WireguardPeerAttribute::PersistentKeepalive(secs));
    }

    // Allowed IPs: comma-separated CIDR list. Replace the kernel's existing set
    // so a re-pushed config is authoritative rather than additive.
    let mut allowed = Vec::new();
    for cidr in peer.allowed_ips.split(',') {
        let cidr = cidr.trim();
        if cidr.is_empty() {
            continue;
        }
        let (ip, prefix) =
            parse_cidr(cidr).map_err(|e| err(format!("peer allowed_ip {cidr:?}: {e}")))?;
        allowed.push(WireguardAllowedIp(vec![
            WireguardAllowedIpAttr::Family(match ip {
                IpAddr::V4(_) => netlink_packet_wireguard::WireguardAddressFamily::Ipv4,
                IpAddr::V6(_) => netlink_packet_wireguard::WireguardAddressFamily::Ipv6,
            }),
            WireguardAllowedIpAttr::IpAddr(ip),
            WireguardAllowedIpAttr::Cidr(prefix),
        ]));
    }
    if !allowed.is_empty() {
        attrs.push(WireguardPeerAttribute::AllowedIps(allowed));
    }

    Ok(WireguardPeer(attrs))
}

// ----------------------------------------------------------------------
// Shared helpers
// ----------------------------------------------------------------------

/// A kernel netlink ACK outcome distinguishing a transport error from a kernel
/// error code, so callers can special-case `EEXIST`.
#[derive(Debug)]
enum NetlinkAckError {
    /// The kernel returned a negative errno in an `NLMSG_ERROR` reply.
    Kernel(i32),
    /// A transport / decode failure.
    Transport(Error),
}

impl std::fmt::Display for NetlinkAckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetlinkAckError::Kernel(code) => write!(f, "kernel error {code}"),
            NetlinkAckError::Transport(e) => write!(f, "{e}"),
        }
    }
}

impl From<Error> for NetlinkAckError {
    fn from(e: Error) -> Self {
        NetlinkAckError::Transport(e)
    }
}

/// Serialize a finalized rtnetlink request, send it, and await its ACK.
///
/// On success (an ACK with code 0) returns `Ok(())`; a kernel error reply is
/// surfaced as [`NetlinkAckError::Kernel`] so the caller can match `EEXIST`.
fn send_route_request(
    mut msg: NetlinkMessage<RouteNetlinkMessage>,
) -> std::result::Result<(), NetlinkAckError> {
    let socket = open_route_socket()?;
    msg.finalize();
    let mut buf = vec![0u8; msg.buffer_len()];
    msg.serialize(&mut buf);
    socket
        .send(&buf, 0)
        .map_err(|e| Error(format!("rtnetlink send: {e}")))?;

    let mut recv_buf = vec![0u8; 8192];
    let n = socket
        .recv(&mut &mut recv_buf[..], 0)
        .map_err(|e| Error(format!("rtnetlink recv: {e}")))?;
    let reply = NetlinkMessage::<RouteNetlinkMessage>::deserialize(&recv_buf[..n])
        .map_err(|e| Error(format!("rtnetlink decode: {e}")))?;
    match reply.payload {
        NetlinkPayload::Error(e) => match e.code {
            // code == None is a bare ACK (success); a non-None code is a
            // negative errno from the kernel.
            None => Ok(()),
            Some(code) => Err(NetlinkAckError::Kernel(code.get())),
        },
        // DONE / unrelated payloads on an ACKed request: treat as success.
        _ => Ok(()),
    }
}

/// Resolve an interface name to its kernel index via `if_nametoindex(3)`.
fn if_nametoindex(ifname: &str) -> Result<u32> {
    let c_name = std::ffi::CString::new(ifname).map_err(|_| err("if_nametoindex: nul in name"))?;
    // SAFETY: `c_name` is a valid NUL-terminated C string for the call.
    let idx = unsafe { libc::if_nametoindex(c_name.as_ptr()) };
    if idx == 0 {
        return Err(err(format!(
            "if_nametoindex({ifname}): {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(idx)
}

/// Decode a base64 `WireGuard` key (44-char base64 of 32 raw bytes) into the
/// fixed-size array the netlink attribute expects.
///
/// A small standard-alphabet base64 decoder is inlined here rather than pulling
/// in a base64 crate: `WireGuard` keys are a fixed, well-formed 32-byte payload, so
/// the decoder only needs to handle the canonical alphabet with `=` padding.
fn decode_wg_key(b64: &str) -> std::result::Result<[u8; WG_KEY_LEN], String> {
    let decoded = base64_decode(b64.trim())?;
    if decoded.len() != WG_KEY_LEN {
        return Err(format!(
            "expected {WG_KEY_LEN}-byte key, decoded {} bytes",
            decoded.len()
        ));
    }
    let mut out = [0u8; WG_KEY_LEN];
    out.copy_from_slice(&decoded);
    Ok(out)
}

/// Minimal RFC 4648 standard-alphabet base64 decoder (with `=` padding).
fn base64_decode(s: &str) -> std::result::Result<Vec<u8>, String> {
    /// Map a base64 alphabet byte to its 6-bit value, or `None` for invalid.
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    let bytes = s.as_bytes();
    // Strip trailing '=' padding.
    let mut end = bytes.len();
    while end > 0 && bytes[end - 1] == b'=' {
        end -= 1;
    }
    let data = &bytes[..end];

    let mut out = Vec::with_capacity(data.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits: u8 = 0;
    for &c in data {
        let v = val(c).ok_or_else(|| format!("invalid base64 char {:?}", c as char))?;
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    Ok(out)
}

/// Parse a CIDR string (`"10.42.0.0/16"`, `"10.42.0.9/32"`, or a bare IP, in
/// which case the prefix is the host length) into `(addr, prefix_len)`.
fn parse_cidr(cidr: &str) -> std::result::Result<(IpAddr, u8), String> {
    let (ip_str, prefix_str) = match cidr.split_once('/') {
        Some((ip, prefix)) => (ip, Some(prefix)),
        None => (cidr, None),
    };
    let ip: IpAddr = ip_str
        .parse()
        .map_err(|e| format!("bad ip {ip_str:?}: {e}"))?;
    let max = match ip {
        IpAddr::V4(_) => 32u8,
        IpAddr::V6(_) => 128u8,
    };
    let prefix = match prefix_str {
        Some(p) => {
            let parsed: u8 = p.parse().map_err(|e| format!("bad prefix {p:?}: {e}"))?;
            if parsed > max {
                return Err(format!("prefix /{parsed} exceeds /{max}"));
            }
            parsed
        }
        None => max,
    };
    Ok((ip, prefix))
}

/// Write overlay DNS into `/etc/resolv.conf` for the workload.
///
/// The agent has already pivoted into the container root, so this path is the
/// resolver file the workload sees. We mirror the udhcpc lease script's format
/// (`nameserver <ip>` lines) and append a `search <domain>` line for the search
/// domain, without clobbering any nameservers a prior DHCP lease wrote — overlay
/// DNS is additive.
fn write_resolv_conf(dns_server: Option<&str>, dns_domain: Option<&str>) -> Result<()> {
    use std::io::Write;

    // Validate the server is a real IP before writing, so we never emit a
    // malformed resolv.conf line.
    if let Some(server) = dns_server {
        let server = server.trim();
        if !server.is_empty() {
            let _ip: IpAddr = server
                .parse()
                .map_err(|e| err(format!("dns_server {server:?}: {e}")))?;
        }
    }

    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/etc/resolv.conf")
        .map_err(|e| err(format!("open /etc/resolv.conf: {e}")))?;

    if let Some(server) = dns_server {
        let server = server.trim();
        if !server.is_empty() {
            writeln!(f, "nameserver {server}")
                .map_err(|e| err(format!("write resolv.conf: {e}")))?;
        }
    }
    if let Some(domain) = dns_domain {
        let domain = domain.trim();
        if !domain.is_empty() {
            writeln!(f, "search {domain}").map_err(|e| err(format!("write resolv.conf: {e}")))?;
        }
    }
    Ok(())
}
