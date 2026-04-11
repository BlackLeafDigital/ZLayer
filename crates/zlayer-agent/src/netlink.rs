//! Rust netlink helpers that replace shell-outs to `ip`/`nsenter`/`sysctl`
//! for per-container overlay network setup.
//!
//! This module is populated incrementally through a phased migration.
//! Stage 1: `move_link_into_netns_and_rename` replaces the shell pair
//!          `ip link set <name> netns <pid>` + `nsenter -t <pid> -n ip
//!          link set <name> name <new>` with a single atomic RTNETLINK
//!          `SetLink` carrying both `IFLA_NET_NS_FD` and `IFLA_IFNAME`.
//!          This bypasses the `/proc/<pid>/ns/net` access problem caused
//!          by libcontainer setting `PR_SET_DUMPABLE(false)` on the
//!          container init process under `SELinux` enforcing.
//! Stage 2: `create_veth_pair`, `delete_link_by_name`, and
//!          `set_link_up_by_name` replace the host-side veth shell
//!          commands (`ip link add ... type veth peer name ...`,
//!          `ip link delete ...`, `ip link set ... up`) used by
//!          `overlay_manager::attach_to_interface` and the orphan
//!          sweeper. These helpers talk RTNETLINK directly via the
//!          `rtnetlink` crate (async, tokio-backed).
//! Stage 3: `with_netns`, `add_address_to_link_by_name`, and
//!          `add_default_route_via_dev` replace the remaining
//!          container-netns shell-outs in
//!          `overlay_manager::attach_to_interface`. `with_netns`
//!          runs a closure on a dedicated OS thread that has joined
//!          the target container's network namespace via `setns(2)`,
//!          while the two new RTNETLINK helpers operate on the
//!          current netns (so they must be invoked from inside a
//!          `with_netns` closure). This removes the last three
//!          `nsenter -t <pid> -n ip ...` shell-outs used to assign
//!          the container IP, bring `eth0` / `lo` up, and add the
//!          default route.

#![cfg_attr(
    not(target_os = "linux"),
    allow(clippy::missing_errors_doc, clippy::unused_async)
)]

use thiserror::Error;

/// Errors returned by the netlink helpers in this module.
#[derive(Debug, Error)]
pub enum NetlinkError {
    /// Failed to open or access a file (typically `/proc/<pid>/ns/net`).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// The requested link was not found in the current network namespace.
    #[error("link '{0}' not found in current netns")]
    NotFound(String),

    /// A netlink operation failed.
    #[error("netlink operation failed: {0}")]
    Netlink(String),
}

/// Move a link from the current network namespace into the network
/// namespace referenced by `ns_fd`, renaming it in the same atomic
/// operation.
///
/// This is the fd-based variant of [`move_link_into_netns_and_rename`].
/// Callers that have already opened `/proc/<pid>/ns/net` (e.g. to pin
/// the namespace across multiple operations and survive a racing
/// container init exit) should use this form so we don't reopen the
/// path and lose the race.
///
/// The single RTNETLINK `SetLink` request carries both `IFLA_NET_NS_FD`
/// and `IFLA_IFNAME`, so the kernel performs the move and the rename
/// atomically.
///
/// # Errors
///
/// Returns [`NetlinkError::NotFound`] if `link_name` does not exist in
/// the current netns. Returns [`NetlinkError::Netlink`] for any other
/// netlink-level failure (permission denied, name collision in the
/// target netns, etc.).
#[cfg(target_os = "linux")]
pub fn move_link_into_netns_fd_and_rename(
    link_name: &str,
    ns_fd: std::os::fd::BorrowedFd<'_>,
    new_name: &str,
) -> Result<(), NetlinkError> {
    use std::os::fd::AsRawFd;

    use libcontainer::network::link::LinkClient;
    use libcontainer::network::wrapper::create_network_client;

    // Build a LinkClient backed by the real rtnetlink socket. If the
    // socket failed to initialize, libcontainer stores an error state
    // and every subsequent call returns `ClientInitializeError`; we
    // surface that through `NetlinkError::Netlink` below.
    let client = create_network_client();
    let mut link_client = LinkClient::new(client)
        .map_err(|e| NetlinkError::Netlink(format!("failed to create LinkClient: {e}")))?;

    // Resolve the host-side interface index. libcontainer returns an
    // error for missing interfaces; map that to our dedicated variant
    // so callers can distinguish "nothing to move" from real failures.
    let link = link_client.get_by_name(link_name).map_err(|e| {
        // libcontainer's NetworkError does not expose a kind we can
        // match on, so we fall back to string inspection. In practice
        // the only expected failure at this stage is ENODEV which
        // manifests as "No such device" from the kernel.
        let msg = e.to_string();
        if msg.contains("No such device") || msg.contains("not found") {
            NetlinkError::NotFound(link_name.to_string())
        } else {
            NetlinkError::Netlink(format!("get_by_name({link_name}) failed: {msg}"))
        }
    })?;

    let index = link.header.index;

    // Atomically move the link into the target netns and rename it.
    // The caller retains ownership of `ns_fd`; `as_raw_fd()` only
    // borrows the raw fd for the duration of the call.
    link_client
        .set_ns_fd(index, new_name, ns_fd.as_raw_fd())
        .map_err(|e| {
            NetlinkError::Netlink(format!(
                "set_ns_fd(index={index}, new_name={new_name}) failed: {e}"
            ))
        })?;

    Ok(())
}

/// Non-Linux stub.
#[cfg(not(target_os = "linux"))]
pub fn move_link_into_netns_fd_and_rename(
    _link_name: &str,
    _ns_fd: std::os::fd::BorrowedFd<'_>,
    _new_name: &str,
) -> Result<(), NetlinkError> {
    Err(NetlinkError::Netlink(
        "move_link_into_netns_fd_and_rename is only supported on Linux".to_string(),
    ))
}

/// Move a link from the current network namespace into the target PID's
/// network namespace, renaming it in the same atomic operation.
///
/// Thin wrapper around [`move_link_into_netns_fd_and_rename`] that
/// opens `/proc/<target_pid>/ns/net` then delegates. Kept for
/// backward compatibility and for callers that only need a single
/// operation on the target netns. Callers that need to perform
/// multiple operations on the same netns (and want to survive a
/// racing exit of the container init process) should open the fd
/// themselves and call [`move_link_into_netns_fd_and_rename`]
/// directly.
///
/// # Errors
///
/// Returns [`NetlinkError::Io`] if `/proc/<target_pid>/ns/net` cannot be
/// opened (e.g. the container process is gone or is not dumpable and we
/// lack `CAP_SYS_PTRACE`). Returns [`NetlinkError::NotFound`] if
/// `link_name` does not exist in the current netns. Returns
/// [`NetlinkError::Netlink`] for any other netlink-level failure
/// (permission denied, name collision in the target netns, etc.).
#[cfg(target_os = "linux")]
pub fn move_link_into_netns_and_rename(
    link_name: &str,
    target_pid: u32,
    new_name: &str,
) -> Result<(), NetlinkError> {
    use std::os::fd::{AsFd, OwnedFd};

    let ns_file = std::fs::File::open(format!("/proc/{target_pid}/ns/net"))?;
    let ns_fd: OwnedFd = OwnedFd::from(ns_file);
    move_link_into_netns_fd_and_rename(link_name, ns_fd.as_fd(), new_name)
}

/// Non-Linux stub: the overlay manager never calls this on non-Linux
/// platforms (libcontainer itself is a Linux-only dep), but keeping the
/// signature available lets `overlay_manager.rs` stay platform-agnostic.
#[cfg(not(target_os = "linux"))]
pub fn move_link_into_netns_and_rename(
    _link_name: &str,
    _target_pid: u32,
    _new_name: &str,
) -> Result<(), NetlinkError> {
    Err(NetlinkError::Netlink(
        "move_link_into_netns_and_rename is only supported on Linux".to_string(),
    ))
}

/// Create a veth pair with the two ends named `host_name` and `peer_name`.
///
/// Both ends start in the current network namespace. The caller is
/// responsible for moving the peer end into the container netns (see
/// [`move_link_into_netns_and_rename`]) and bringing the host end up
/// (see [`set_link_up_by_name`]).
///
/// Replaces the shell-out:
///   ip link add `<host_name>` type veth peer name `<peer_name>`
///
/// # Errors
///
/// Returns [`NetlinkError::Netlink`] if RTNETLINK fails for any
/// reason. `EEXIST` / "File exists" is surfaced verbatim so the caller
/// can distinguish a leaked endpoint (typically a sign the orphan
/// sweeper missed something) from a permission or interface-name
/// problem.
#[cfg(target_os = "linux")]
pub async fn create_veth_pair(host_name: &str, peer_name: &str) -> Result<(), NetlinkError> {
    let (connection, handle, _) = rtnetlink::new_connection()
        .map_err(|e| NetlinkError::Netlink(format!("new_connection failed: {e}")))?;
    tokio::spawn(connection);

    handle
        .link()
        .add()
        .veth(host_name.to_string(), peer_name.to_string())
        .execute()
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("File exists") || msg.contains("EEXIST") {
                NetlinkError::Netlink(format!(
                    "veth pair already exists: host={host_name} peer={peer_name}: {msg}"
                ))
            } else {
                NetlinkError::Netlink(format!(
                    "veth create failed (host={host_name}, peer={peer_name}): {msg}"
                ))
            }
        })
}

/// Non-Linux stub.
#[cfg(not(target_os = "linux"))]
pub async fn create_veth_pair(_host_name: &str, _peer_name: &str) -> Result<(), NetlinkError> {
    Err(NetlinkError::Netlink(
        "create_veth_pair is only supported on Linux".to_string(),
    ))
}

/// Delete the link by name. Idempotent: returns `Ok(())` if the link
/// does not exist. Any other error surfaces as
/// [`NetlinkError::Netlink`].
///
/// Replaces the shell-out:
///   ip link delete `<name>`
///
/// Used in `overlay_manager::attach_to_interface` pre-cleanup,
/// cleanup-on-error, and the orphan-veth sweeper.
///
/// # Errors
///
/// Returns [`NetlinkError::Netlink`] if RTNETLINK reports a failure
/// other than `ENODEV` / "No such device" (which are treated as
/// success so this is safe to call unconditionally).
#[cfg(target_os = "linux")]
pub async fn delete_link_by_name(name: &str) -> Result<(), NetlinkError> {
    use futures_util::stream::TryStreamExt;

    let (connection, handle, _) = rtnetlink::new_connection()
        .map_err(|e| NetlinkError::Netlink(format!("new_connection failed: {e}")))?;
    tokio::spawn(connection);

    // Look up the link by name. Treat "not found" as success so the
    // helper is safe to call unconditionally in cleanup paths.
    let lookup = handle
        .link()
        .get()
        .match_name(name.to_string())
        .execute()
        .try_next()
        .await;

    let link = match lookup {
        Ok(Some(link)) => link,
        Ok(None) => return Ok(()),
        Err(rtnetlink::Error::NetlinkError(err)) => {
            // libc::ENODEV == 19. netlink-packet-core reports the raw
            // errno as a negative i32 in `code`, but the exact type has
            // moved between versions, so match by both numeric code and
            // the human-readable message for belt-and-suspenders safety.
            let msg = err.to_string();
            let is_enodev = err
                .code
                .is_some_and(|c| c.get().unsigned_abs() == libc::ENODEV as u32);
            if is_enodev || msg.contains("No such device") {
                return Ok(());
            }
            return Err(NetlinkError::Netlink(format!(
                "link lookup failed for {name}: {msg}"
            )));
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("No such device") {
                return Ok(());
            }
            return Err(NetlinkError::Netlink(format!(
                "link lookup failed for {name}: {msg}"
            )));
        }
    };

    let index = link.header.index;

    handle
        .link()
        .del(index)
        .execute()
        .await
        .map_err(|e| NetlinkError::Netlink(format!("link delete failed for {name}: {e}")))
}

/// Non-Linux stub.
#[cfg(not(target_os = "linux"))]
pub async fn delete_link_by_name(_name: &str) -> Result<(), NetlinkError> {
    Err(NetlinkError::Netlink(
        "delete_link_by_name is only supported on Linux".to_string(),
    ))
}

/// List all network interfaces in the current netns.
///
/// Returns a `Vec` of `(index, name)` tuples for every link the kernel
/// reports. Used by the orphan veth sweeper to find `veth-<pid>` and
/// `vc-<pid>` links whose owning PID is dead, so it can clean them up
/// via [`delete_link_by_name`].
///
/// Replaces the shell-out:
///   ip -br link
///
/// Issues a single RTNETLINK `RTM_GETLINK` dump request and iterates
/// the resulting stream of `LinkMessage`s. Each message contributes
/// one `(index, name)` tuple; messages without an `IFLA_IFNAME`
/// attribute (extremely rare in practice — the kernel always emits
/// one for configured devices) are silently skipped.
///
/// # Errors
///
/// Returns [`NetlinkError::Netlink`] if the rtnetlink socket cannot
/// be created or if the dump stream itself reports a failure.
#[cfg(target_os = "linux")]
pub async fn list_all_links() -> Result<Vec<(u32, String)>, NetlinkError> {
    use futures_util::stream::TryStreamExt;
    use netlink_packet_route::link::LinkAttribute;

    let (connection, handle, _) = rtnetlink::new_connection()
        .map_err(|e| NetlinkError::Netlink(format!("new_connection failed: {e}")))?;
    tokio::spawn(connection);

    let mut stream = handle.link().get().execute();
    let mut links = Vec::new();

    while let Some(msg) = stream
        .try_next()
        .await
        .map_err(|e| NetlinkError::Netlink(format!("link dump failed: {e}")))?
    {
        // LinkHeader.index is already u32 in netlink-packet-route
        // 0.19 — no cast needed.
        let index = msg.header.index;
        let Some(name) = msg.attributes.iter().find_map(|a| match a {
            LinkAttribute::IfName(n) => Some(n.clone()),
            _ => None,
        }) else {
            continue;
        };
        links.push((index, name));
    }

    Ok(links)
}

/// Non-Linux stub.
#[cfg(not(target_os = "linux"))]
pub async fn list_all_links() -> Result<Vec<(u32, String)>, NetlinkError> {
    Err(NetlinkError::Netlink(
        "list_all_links is only supported on Linux".to_string(),
    ))
}

/// Set the link identified by `name` to the "up" administrative state.
///
/// Replaces the shell-out:
///   ip link set `<name>` up
///
/// Unlike [`delete_link_by_name`] this is *not* idempotent for missing
/// links: if the link does not exist the caller almost certainly has a
/// bug upstream (we only call this on a veth end we just created), so
/// we return [`NetlinkError::NotFound`] rather than silently succeeding.
///
/// # Errors
///
/// Returns [`NetlinkError::NotFound`] if no link with the given name
/// exists in the current netns. Returns [`NetlinkError::Netlink`] for
/// any other RTNETLINK failure (permission denied, etc.).
#[cfg(target_os = "linux")]
pub async fn set_link_up_by_name(name: &str) -> Result<(), NetlinkError> {
    use futures_util::stream::TryStreamExt;

    let (connection, handle, _) = rtnetlink::new_connection()
        .map_err(|e| NetlinkError::Netlink(format!("new_connection failed: {e}")))?;
    tokio::spawn(connection);

    let link = handle
        .link()
        .get()
        .match_name(name.to_string())
        .execute()
        .try_next()
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("No such device") {
                NetlinkError::NotFound(name.to_string())
            } else {
                NetlinkError::Netlink(format!("link lookup failed for {name}: {msg}"))
            }
        })?
        .ok_or_else(|| NetlinkError::NotFound(name.to_string()))?;

    let index = link.header.index;

    handle
        .link()
        .set(index)
        .up()
        .execute()
        .await
        .map_err(|e| NetlinkError::Netlink(format!("link set up failed for {name}: {e}")))
}

/// Non-Linux stub.
#[cfg(not(target_os = "linux"))]
pub async fn set_link_up_by_name(_name: &str) -> Result<(), NetlinkError> {
    Err(NetlinkError::Netlink(
        "set_link_up_by_name is only supported on Linux".to_string(),
    ))
}

/// Add an IP address to the link identified by `name` in the current
/// network namespace.
///
/// Replaces (in combination with [`with_netns`]):
///   nsenter -t `<pid>` -n ip \[-6\] addr add `<addr>/<prefix_len>` dev `<name>`
///
/// `addr` may be v4 or v6. `prefix_len` is the CIDR prefix length
/// (24 for a `/24`, 64 for a `/64`, etc.).
///
/// This helper operates on the CURRENT network namespace — it looks
/// up the interface index via a local rtnetlink socket. To target a
/// container's netns, wrap the call inside [`with_netns`].
///
/// # Errors
///
/// Returns [`NetlinkError::NotFound`] if the link is missing. Returns
/// [`NetlinkError::Netlink`] for any other rtnetlink failure
/// (permission denied, EEXIST on a duplicate address, etc.).
#[cfg(target_os = "linux")]
pub async fn add_address_to_link_by_name(
    name: &str,
    addr: std::net::IpAddr,
    prefix_len: u8,
) -> Result<(), NetlinkError> {
    use futures_util::stream::TryStreamExt;

    let (connection, handle, _) = rtnetlink::new_connection()
        .map_err(|e| NetlinkError::Netlink(format!("new_connection failed: {e}")))?;
    tokio::spawn(connection);

    let link = handle
        .link()
        .get()
        .match_name(name.to_string())
        .execute()
        .try_next()
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("No such device") {
                NetlinkError::NotFound(name.to_string())
            } else {
                NetlinkError::Netlink(format!("link lookup failed for {name}: {msg}"))
            }
        })?
        .ok_or_else(|| NetlinkError::NotFound(name.to_string()))?;

    let index = link.header.index;

    handle
        .address()
        .add(index, addr, prefix_len)
        .execute()
        .await
        .map_err(|e| {
            NetlinkError::Netlink(format!(
                "address add failed for {name} ({addr}/{prefix_len}): {e}"
            ))
        })
}

/// Non-Linux stub.
#[cfg(not(target_os = "linux"))]
pub async fn add_address_to_link_by_name(
    _name: &str,
    _addr: std::net::IpAddr,
    _prefix_len: u8,
) -> Result<(), NetlinkError> {
    Err(NetlinkError::Netlink(
        "add_address_to_link_by_name is only supported on Linux".to_string(),
    ))
}

/// Add a default route via the given device name in the current
/// network namespace.
///
/// Replaces (in combination with [`with_netns`]):
///   nsenter -t `<pid>` -n ip \[-6\] route add default dev `<dev_name>`
///
/// The route is a direct, link-scope route: no gateway, the kernel
/// ARPs / uses NDISC on the device for destination resolution. This
/// is the correct form for a point-to-point veth link where the peer
/// is reachable directly.
///
/// For IPv4 the destination prefix is `0.0.0.0/0`. For IPv6 it is
/// `::/0`. Controlled by `is_v6`.
///
/// # Errors
///
/// Returns [`NetlinkError::NotFound`] if the device is missing.
/// Returns [`NetlinkError::Netlink`] for any other rtnetlink failure.
#[cfg(target_os = "linux")]
pub async fn add_default_route_via_dev(dev_name: &str, is_v6: bool) -> Result<(), NetlinkError> {
    use futures_util::stream::TryStreamExt;
    use netlink_packet_route::route::RouteScope;

    let (connection, handle, _) = rtnetlink::new_connection()
        .map_err(|e| NetlinkError::Netlink(format!("new_connection failed: {e}")))?;
    tokio::spawn(connection);

    let link = handle
        .link()
        .get()
        .match_name(dev_name.to_string())
        .execute()
        .try_next()
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("No such device") {
                NetlinkError::NotFound(dev_name.to_string())
            } else {
                NetlinkError::Netlink(format!("link lookup failed for {dev_name}: {msg}"))
            }
        })?
        .ok_or_else(|| NetlinkError::NotFound(dev_name.to_string()))?;

    let oif_idx = link.header.index;

    if is_v6 {
        handle
            .route()
            .add()
            .v6()
            .destination_prefix(std::net::Ipv6Addr::UNSPECIFIED, 0)
            .output_interface(oif_idx)
            .scope(RouteScope::Link)
            .execute()
            .await
            .map_err(|e| {
                NetlinkError::Netlink(format!("default route add v6 via {dev_name} failed: {e}"))
            })
    } else {
        handle
            .route()
            .add()
            .v4()
            .destination_prefix(std::net::Ipv4Addr::UNSPECIFIED, 0)
            .output_interface(oif_idx)
            .scope(RouteScope::Link)
            .execute()
            .await
            .map_err(|e| {
                NetlinkError::Netlink(format!("default route add v4 via {dev_name} failed: {e}"))
            })
    }
}

/// Non-Linux stub.
#[cfg(not(target_os = "linux"))]
pub async fn add_default_route_via_dev(_dev_name: &str, _is_v6: bool) -> Result<(), NetlinkError> {
    Err(NetlinkError::Netlink(
        "add_default_route_via_dev is only supported on Linux".to_string(),
    ))
}

/// Add or replace a route to `dest/prefix_len` that forwards via the
/// interface named `dev_name`. Optional `src` sets the preferred source
/// address.
///
/// Replaces the shell-outs:
///   ip route replace `<dest>/<prefix_len>` dev `<dev_name>` \[src `<src>`\]
///   ip -6 route replace `<dest>/<prefix_len>` dev `<dev_name>` \[src `<src>`\]
///
/// Uses `NLM_F_REPLACE | NLM_F_CREATE` semantics (via rtnetlink's
/// `.replace()` on the route add builder) so stale routes left behind
/// by a previous daemon run don't cause `EEXIST`.
///
/// The route is installed with link scope (direct-via-dev, no
/// gateway) which is the correct form for a per-container `/32` or
/// `/128` pointing at a host-side veth endpoint.
///
/// `dest` and `src` (if provided) must have matching address families
/// — passing a v4 `dest` with a v6 `src` returns
/// [`NetlinkError::Netlink`] without touching the kernel.
///
/// # Errors
///
/// Returns [`NetlinkError::NotFound`] if `dev_name` does not exist in
/// the current netns. Returns [`NetlinkError::Netlink`] on address
/// family mismatch or any RTNETLINK failure.
#[cfg(target_os = "linux")]
pub async fn replace_route_via_dev(
    dest: std::net::IpAddr,
    prefix_len: u8,
    dev_name: &str,
    src: Option<std::net::IpAddr>,
) -> Result<(), NetlinkError> {
    use std::net::IpAddr;

    use futures_util::stream::TryStreamExt;
    use netlink_packet_route::route::RouteScope;

    let (connection, handle, _) = rtnetlink::new_connection()
        .map_err(|e| NetlinkError::Netlink(format!("new_connection failed: {e}")))?;
    tokio::spawn(connection);

    let link = handle
        .link()
        .get()
        .match_name(dev_name.to_string())
        .execute()
        .try_next()
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("No such device") {
                NetlinkError::NotFound(dev_name.to_string())
            } else {
                NetlinkError::Netlink(format!("link lookup failed for {dev_name}: {msg}"))
            }
        })?
        .ok_or_else(|| NetlinkError::NotFound(dev_name.to_string()))?;

    let oif_idx = link.header.index;

    match (dest, src) {
        (IpAddr::V4(d), Some(IpAddr::V4(s))) => handle
            .route()
            .add()
            .v4()
            .destination_prefix(d, prefix_len)
            .output_interface(oif_idx)
            .scope(RouteScope::Link)
            .pref_source(s)
            .replace()
            .execute()
            .await
            .map_err(|e| {
                NetlinkError::Netlink(format!(
                    "route replace v4 {d}/{prefix_len} dev {dev_name} src {s} failed: {e}"
                ))
            }),
        (IpAddr::V4(d), None) => handle
            .route()
            .add()
            .v4()
            .destination_prefix(d, prefix_len)
            .output_interface(oif_idx)
            .scope(RouteScope::Link)
            .replace()
            .execute()
            .await
            .map_err(|e| {
                NetlinkError::Netlink(format!(
                    "route replace v4 {d}/{prefix_len} dev {dev_name} failed: {e}"
                ))
            }),
        (IpAddr::V6(d), Some(IpAddr::V6(s))) => handle
            .route()
            .add()
            .v6()
            .destination_prefix(d, prefix_len)
            .output_interface(oif_idx)
            .scope(RouteScope::Link)
            .pref_source(s)
            .replace()
            .execute()
            .await
            .map_err(|e| {
                NetlinkError::Netlink(format!(
                    "route replace v6 {d}/{prefix_len} dev {dev_name} src {s} failed: {e}"
                ))
            }),
        (IpAddr::V6(d), None) => handle
            .route()
            .add()
            .v6()
            .destination_prefix(d, prefix_len)
            .output_interface(oif_idx)
            .scope(RouteScope::Link)
            .replace()
            .execute()
            .await
            .map_err(|e| {
                NetlinkError::Netlink(format!(
                    "route replace v6 {d}/{prefix_len} dev {dev_name} failed: {e}"
                ))
            }),
        (IpAddr::V4(_), Some(IpAddr::V6(_))) | (IpAddr::V6(_), Some(IpAddr::V4(_))) => Err(
            NetlinkError::Netlink(format!("address family mismatch: dest={dest} src={src:?}")),
        ),
    }
}

/// Non-Linux stub.
#[cfg(not(target_os = "linux"))]
pub async fn replace_route_via_dev(
    _dest: std::net::IpAddr,
    _prefix_len: u8,
    _dev_name: &str,
    _src: Option<std::net::IpAddr>,
) -> Result<(), NetlinkError> {
    Err(NetlinkError::Netlink(
        "replace_route_via_dev is only supported on Linux".to_string(),
    ))
}

/// Set a sysctl via the `/proc/sys/...` filesystem.
///
/// `key` uses dotted form like `net.ipv4.ip_forward`; dots are
/// translated to path separators so the effective path is
/// `/proc/sys/net/ipv4/ip_forward`. Writes the string form of
/// `value` to the file.
///
/// Replaces the shell-outs:
///   sysctl -w `<key>`=`<value>`
///
/// Writing to `/proc/sys/...` is the kernel-standard way of setting
/// sysctls and works under any confinement that still allows write
/// access to `/proc/sys` (which the overlay manager needs anyway for
/// its other operations).
///
/// # Errors
///
/// Returns [`NetlinkError::Io`] if the write fails (e.g. permission
/// denied, file missing because the sysctl doesn't exist on this
/// kernel, etc.).
pub fn set_sysctl(key: &str, value: &str) -> Result<(), NetlinkError> {
    let path = format!("/proc/sys/{}", key.replace('.', "/"));
    std::fs::write(&path, value)?;
    Ok(())
}

/// Run a synchronous closure inside the network namespace referenced
/// by the given `OwnedFd`.
///
/// This is the fd-based variant of [`with_netns`]. Callers that have
/// already opened `/proc/<pid>/ns/net` (e.g. to pin the namespace
/// across multiple operations) should use this form to reuse the
/// same fd and avoid re-opening the procfs path — the reopen would
/// fail with `ENOENT` if the container init process has exited in
/// the meantime, even though the namespace itself is still alive
/// because our pinned fd holds a reference.
///
/// The `OwnedFd` is moved into the dedicated worker thread and
/// closed when the thread exits. Spawns a fresh OS thread (not a
/// tokio blocking worker) because `setns` affects the whole thread
/// and we don't want to contaminate a shared worker.
///
/// # Errors
///
/// Returns [`NetlinkError::Netlink`] if `setns` fails or the
/// dedicated thread panics. Any error returned by the closure itself
/// is propagated verbatim.
#[cfg(target_os = "linux")]
pub fn with_netns_fd<F, T>(ns_fd: std::os::fd::OwnedFd, f: F) -> Result<T, NetlinkError>
where
    F: FnOnce() -> Result<T, NetlinkError> + Send + 'static,
    T: Send + 'static,
{
    let join_handle = std::thread::spawn(move || -> Result<T, NetlinkError> {
        nix::sched::setns(&ns_fd, nix::sched::CloneFlags::CLONE_NEWNET)
            .map_err(|e| NetlinkError::Netlink(format!("setns(ns_fd) failed: {e}")))?;
        // Keep the fd alive for the duration of the closure even
        // though setns only needs it for the syscall itself. Dropping
        // it explicitly after the closure makes the lifetime obvious.
        let result = f();
        drop(ns_fd);
        result
    });

    join_handle
        .join()
        .map_err(|_| NetlinkError::Netlink("with_netns_fd thread panicked".to_string()))?
}

/// Non-Linux stub.
#[cfg(not(target_os = "linux"))]
pub fn with_netns_fd<F, T>(_ns_fd: std::os::fd::OwnedFd, _f: F) -> Result<T, NetlinkError>
where
    F: FnOnce() -> Result<T, NetlinkError> + Send + 'static,
    T: Send + 'static,
{
    Err(NetlinkError::Netlink(
        "with_netns_fd is only supported on Linux".to_string(),
    ))
}

/// Run a synchronous closure inside the network namespace of the
/// given PID.
///
/// Thin wrapper around [`with_netns_fd`] that opens
/// `/proc/<target_pid>/ns/net` then delegates. Kept for backward
/// compatibility and for callers that only need a single operation
/// on the target netns. Callers that need to pin the namespace
/// across multiple operations (and survive a racing exit of the
/// container init) should open the fd themselves and call
/// [`with_netns_fd`] directly.
///
/// Because `setns` is synchronous and `rtnetlink` is async, the
/// typical usage pattern inside the closure is to build a local
/// current-thread tokio runtime and `block_on` the netlink calls.
/// See [`with_netns_async`] for a convenience wrapper that does
/// exactly this.
///
/// # Errors
///
/// Returns [`NetlinkError::Io`] if `/proc/<target_pid>/ns/net` cannot
/// be opened. Returns [`NetlinkError::Netlink`] if `setns` fails or
/// the dedicated thread panics. Any error returned by the closure
/// itself is propagated verbatim.
#[cfg(target_os = "linux")]
pub fn with_netns<F, T>(target_pid: u32, f: F) -> Result<T, NetlinkError>
where
    F: FnOnce() -> Result<T, NetlinkError> + Send + 'static,
    T: Send + 'static,
{
    use std::os::fd::OwnedFd;

    let ns_file = std::fs::File::open(format!("/proc/{target_pid}/ns/net"))?;
    let ns_fd: OwnedFd = OwnedFd::from(ns_file);
    with_netns_fd(ns_fd, f)
}

/// Non-Linux stub.
#[cfg(not(target_os = "linux"))]
pub fn with_netns<F, T>(_target_pid: u32, _f: F) -> Result<T, NetlinkError>
where
    F: FnOnce() -> Result<T, NetlinkError> + Send + 'static,
    T: Send + 'static,
{
    Err(NetlinkError::Netlink(
        "with_netns is only supported on Linux".to_string(),
    ))
}

/// Convenience wrapper around [`with_netns_fd`] that builds a local
/// current-thread tokio runtime inside the dedicated thread and
/// drives the provided async future to completion.
///
/// The future is produced by calling `f()` from inside the thread
/// that has already joined the target netns, so any rtnetlink
/// operations awaited inside the future will talk to the target
/// netns's kernel.
///
/// The local runtime is lightweight (single-thread, built per call)
/// and only drives a handful of netlink messages before being
/// dropped with the thread.
///
/// The `OwnedFd` is moved into the worker thread and closed when
/// the thread exits.
///
/// # Errors
///
/// Returns [`NetlinkError::Netlink`] per [`with_netns_fd`], plus
/// [`NetlinkError::Netlink`] if the local runtime fails to build.
/// Any error returned by the future is propagated verbatim.
#[cfg(target_os = "linux")]
pub fn with_netns_fd_async<F, Fut, T>(ns_fd: std::os::fd::OwnedFd, f: F) -> Result<T, NetlinkError>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T, NetlinkError>>,
    T: Send + 'static,
{
    with_netns_fd(ns_fd, move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| NetlinkError::Netlink(format!("local runtime build failed: {e}")))?;
        rt.block_on(f())
    })
}

/// Non-Linux stub.
#[cfg(not(target_os = "linux"))]
pub fn with_netns_fd_async<F, Fut, T>(
    _ns_fd: std::os::fd::OwnedFd,
    _f: F,
) -> Result<T, NetlinkError>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T, NetlinkError>>,
    T: Send + 'static,
{
    Err(NetlinkError::Netlink(
        "with_netns_fd_async is only supported on Linux".to_string(),
    ))
}

/// Convenience wrapper around [`with_netns`] that builds a local
/// current-thread tokio runtime inside the dedicated thread and
/// drives the provided async future to completion.
///
/// Thin wrapper around [`with_netns_fd_async`] that opens
/// `/proc/<target_pid>/ns/net` then delegates.
///
/// # Errors
///
/// Returns [`NetlinkError::Io`] / [`NetlinkError::Netlink`] per
/// [`with_netns`], plus [`NetlinkError::Netlink`] if the local
/// runtime fails to build. Any error returned by the future is
/// propagated verbatim.
#[cfg(target_os = "linux")]
pub fn with_netns_async<F, Fut, T>(target_pid: u32, f: F) -> Result<T, NetlinkError>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T, NetlinkError>>,
    T: Send + 'static,
{
    use std::os::fd::OwnedFd;

    let ns_file = std::fs::File::open(format!("/proc/{target_pid}/ns/net"))?;
    let ns_fd: OwnedFd = OwnedFd::from(ns_file);
    with_netns_fd_async(ns_fd, f)
}

/// Non-Linux stub.
#[cfg(not(target_os = "linux"))]
pub fn with_netns_async<F, Fut, T>(_target_pid: u32, _f: F) -> Result<T, NetlinkError>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T, NetlinkError>>,
    T: Send + 'static,
{
    Err(NetlinkError::Netlink(
        "with_netns_async is only supported on Linux".to_string(),
    ))
}
