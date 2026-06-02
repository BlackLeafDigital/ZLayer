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
///
/// Implemented directly against the `rtnetlink` crate (overlayd has no
/// libcontainer dependency): a single `LinkSetRequest` carrying
/// `setns_by_fd` + `name` performs the move and rename atomically.
#[cfg(target_os = "linux")]
pub fn move_link_into_netns_fd_and_rename(
    link_name: &str,
    ns_fd: std::os::fd::BorrowedFd<'_>,
    new_name: &str,
) -> Result<(), NetlinkError> {
    use std::os::fd::AsRawFd;

    // `setns` of the moved link must reference the fd while the request
    // executes, so we drive the whole sequence on a local current-thread
    // runtime rather than requiring an ambient tokio context. The raw fd
    // is borrowed (the caller retains ownership of `ns_fd`).
    let raw_fd = ns_fd.as_raw_fd();
    let link_name = link_name.to_string();
    let new_name = new_name.to_string();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| NetlinkError::Netlink(format!("local runtime build failed: {e}")))?;

    rt.block_on(async move {
        use futures_util::stream::TryStreamExt;

        let (connection, handle, _) = rtnetlink::new_connection()
            .map_err(|e| NetlinkError::Netlink(format!("new_connection failed: {e}")))?;
        tokio::spawn(connection);

        // Resolve the host-side interface index. Treat "No such device"
        // as our dedicated NotFound variant so callers can distinguish
        // "nothing to move" from real failures.
        let link = handle
            .link()
            .get()
            .match_name(link_name.clone())
            .execute()
            .try_next()
            .await
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("No such device") {
                    NetlinkError::NotFound(link_name.clone())
                } else {
                    NetlinkError::Netlink(format!("link lookup failed for {link_name}: {msg}"))
                }
            })?
            .ok_or_else(|| NetlinkError::NotFound(link_name.clone()))?;

        let index = link.header.index;

        // Atomically move the link into the target netns and rename it.
        handle
            .link()
            .set(index)
            .setns_by_fd(raw_fd)
            .name(new_name.clone())
            .execute()
            .await
            .map_err(|e| {
                NetlinkError::Netlink(format!(
                    "setns_by_fd(index={index}, new_name={new_name}) failed: {e}"
                ))
            })
    })
}

/// Stub for non-Linux Unix platforms (macOS/BSD).
///
/// Not emitted on Windows: `attach_container` (the sole caller chain) is
/// itself gated `#[cfg(target_os = "linux")]` in `server.rs`, so there are
/// no Windows callers, and the `BorrowedFd` parameter type is Unix-only.
///
/// # Errors
///
/// Always returns [`NetlinkError::Netlink`] — this function is unsupported on
/// the current target.
#[cfg(all(not(target_os = "linux"), unix))]
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

/// Add a default route pointing at the given gateway IP in the current
/// network namespace.
///
/// Replaces (in combination with [`with_netns`]):
///   nsenter -t `<pid>` -n ip \[-6\] route add default via `<gateway>`
///
/// Used by the per-service bridge attach path: containers join the
/// service bridge via a veth pair and reach the rest of the overlay
/// through the bridge's L3 gateway IP. The address family of the route
/// is inferred from `gateway`.
///
/// # Errors
///
/// Returns [`NetlinkError::Netlink`] for any rtnetlink failure.
#[cfg(target_os = "linux")]
pub async fn add_default_route_via_gateway(gateway: std::net::IpAddr) -> Result<(), NetlinkError> {
    let (connection, handle, _) = rtnetlink::new_connection()
        .map_err(|e| NetlinkError::Netlink(format!("new_connection failed: {e}")))?;
    tokio::spawn(connection);

    match gateway {
        std::net::IpAddr::V4(gw) => handle
            .route()
            .add()
            .v4()
            .destination_prefix(std::net::Ipv4Addr::UNSPECIFIED, 0)
            .gateway(gw)
            .execute()
            .await
            .map_err(|e| {
                NetlinkError::Netlink(format!("default route add v4 via gateway {gw} failed: {e}"))
            }),
        std::net::IpAddr::V6(gw) => handle
            .route()
            .add()
            .v6()
            .destination_prefix(std::net::Ipv6Addr::UNSPECIFIED, 0)
            .gateway(gw)
            .execute()
            .await
            .map_err(|e| {
                NetlinkError::Netlink(format!("default route add v6 via gateway {gw} failed: {e}"))
            }),
    }
}

/// Non-Linux stub.
#[cfg(not(target_os = "linux"))]
pub async fn add_default_route_via_gateway(_gateway: std::net::IpAddr) -> Result<(), NetlinkError> {
    Err(NetlinkError::Netlink(
        "add_default_route_via_gateway is only supported on Linux".to_string(),
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

/// Non-Linux Unix (macOS/BSD) stub. Not emitted on Windows — the sole caller
/// chain (`attach_to_interface` in `overlay_manager.rs`) is
/// `#[cfg(target_os = "linux")]`-gated, and `OwnedFd` is Unix-only.
#[cfg(all(not(target_os = "linux"), unix))]
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

/// Non-Linux Unix (macOS/BSD) stub. Not emitted on Windows — the sole caller
/// chain (`attach_to_interface` in `overlay_manager.rs`) is
/// `#[cfg(target_os = "linux")]`-gated, and `OwnedFd` is Unix-only.
#[cfg(all(not(target_os = "linux"), unix))]
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

/// Create a Linux bridge interface with the given name.
///
/// Replaces the shell-out:
///   ip link add name `<name>` type bridge
///
/// Idempotent: if a link with that name already exists this returns
/// `Ok(())`. This matches how the overlay manager's per-service bridge
/// creation path needs to behave — multiple containers landing on the
/// same service-on-node bridge must all see "bridge ready" after a
/// successful call without racing against existence checks.
///
/// The bridge is created in the current network namespace. Callers
/// that need a different netns should wrap with [`with_netns_async`].
/// The bridge is created in the administratively-down state — call
/// [`set_link_up_by_name`] separately once any other attributes
/// ([`set_bridge_stp`] etc.) have been applied.
///
/// # Errors
///
/// Returns [`NetlinkError::Netlink`] for any RTNETLINK failure other
/// than `EEXIST` (which is treated as success).
#[cfg(target_os = "linux")]
pub async fn create_bridge(name: &str) -> Result<(), NetlinkError> {
    let (connection, handle, _) = rtnetlink::new_connection()
        .map_err(|e| NetlinkError::Netlink(format!("new_connection failed: {e}")))?;
    tokio::spawn(connection);

    match handle.link().add().bridge(name.to_string()).execute().await {
        Ok(()) => Ok(()),
        Err(rtnetlink::Error::NetlinkError(err)) => {
            // EEXIST means a link with this name already exists. We
            // intentionally do NOT verify that the existing link is
            // actually a bridge — callers using stable per-service
            // names own that invariant, and re-checking here would
            // require another rtnetlink round-trip on the hot path.
            let is_eexist = err
                .code
                .is_some_and(|c| c.get().unsigned_abs() == libc::EEXIST as u32);
            let msg = err.to_string();
            if is_eexist || msg.contains("File exists") {
                Ok(())
            } else {
                Err(NetlinkError::Netlink(format!(
                    "bridge create failed for {name}: {msg}"
                )))
            }
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("File exists") {
                Ok(())
            } else {
                Err(NetlinkError::Netlink(format!(
                    "bridge create failed for {name}: {msg}"
                )))
            }
        }
    }
}

/// Non-Linux stub.
#[cfg(not(target_os = "linux"))]
pub async fn create_bridge(_name: &str) -> Result<(), NetlinkError> {
    Err(NetlinkError::Netlink(
        "create_bridge is only supported on Linux".to_string(),
    ))
}

/// Delete the bridge interface with the given name.
///
/// Replaces the shell-out:
///   ip link delete `<name>` type bridge
///
/// Idempotent: returns `Ok(())` if the bridge does not exist.
/// Delegates to [`delete_link_by_name`] — from RTNETLINK's perspective
/// deleting a bridge is the same `RTM_DELLINK` as deleting any other
/// link, and `delete_link_by_name` already has the ENODEV-as-success
/// handling we want.
///
/// # Errors
///
/// Returns [`NetlinkError::Netlink`] for any RTNETLINK failure other
/// than `ENODEV` (which is treated as success).
#[cfg(target_os = "linux")]
pub async fn delete_bridge(name: &str) -> Result<(), NetlinkError> {
    delete_link_by_name(name).await
}

/// Non-Linux stub.
#[cfg(not(target_os = "linux"))]
pub async fn delete_bridge(_name: &str) -> Result<(), NetlinkError> {
    Err(NetlinkError::Netlink(
        "delete_bridge is only supported on Linux".to_string(),
    ))
}

/// Attach `link` to `bridge` by setting the link's `IFLA_MASTER` to
/// the bridge's ifindex.
///
/// Replaces the shell-out:
///   ip link set `<link>` master `<bridge>`
///
/// Both interfaces must already exist in the current network
/// namespace. This is what the overlay manager will call to splice a
/// container's host-side veth end into the per-service bridge instead
/// of /32-routing it directly.
///
/// # Errors
///
/// Returns [`NetlinkError::NotFound`] if either `link` or `bridge`
/// does not exist in the current netns. Returns
/// [`NetlinkError::Netlink`] for any other RTNETLINK failure.
#[cfg(target_os = "linux")]
pub async fn add_link_to_bridge(link: &str, bridge: &str) -> Result<(), NetlinkError> {
    use futures_util::stream::TryStreamExt;

    let (connection, handle, _) = rtnetlink::new_connection()
        .map_err(|e| NetlinkError::Netlink(format!("new_connection failed: {e}")))?;
    tokio::spawn(connection);

    let bridge_link = handle
        .link()
        .get()
        .match_name(bridge.to_string())
        .execute()
        .try_next()
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("No such device") {
                NetlinkError::NotFound(bridge.to_string())
            } else {
                NetlinkError::Netlink(format!("link lookup failed for {bridge}: {msg}"))
            }
        })?
        .ok_or_else(|| NetlinkError::NotFound(bridge.to_string()))?;
    let bridge_idx = bridge_link.header.index;

    let member_link = handle
        .link()
        .get()
        .match_name(link.to_string())
        .execute()
        .try_next()
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("No such device") {
                NetlinkError::NotFound(link.to_string())
            } else {
                NetlinkError::Netlink(format!("link lookup failed for {link}: {msg}"))
            }
        })?
        .ok_or_else(|| NetlinkError::NotFound(link.to_string()))?;
    let member_idx = member_link.header.index;

    handle
        .link()
        .set(member_idx)
        .controller(bridge_idx)
        .execute()
        .await
        .map_err(|e| {
            NetlinkError::Netlink(format!(
                "set master failed: link={link} bridge={bridge}: {e}"
            ))
        })
}

/// Non-Linux stub.
#[cfg(not(target_os = "linux"))]
pub async fn add_link_to_bridge(_link: &str, _bridge: &str) -> Result<(), NetlinkError> {
    Err(NetlinkError::Netlink(
        "add_link_to_bridge is only supported on Linux".to_string(),
    ))
}

/// Enable or disable Spanning Tree Protocol (STP) on the named bridge.
///
/// STP is disabled by default on bridges created via [`create_bridge`]
/// (the kernel default for a freshly-created bridge is STP off), and
/// for `ZLayer`'s per-service bridges we want to keep it off: each
/// bridge is single-host, has no possibility of a loop, and STP's
/// initial 30s forwarding-delay would stall container traffic on
/// attach.
///
/// rtnetlink 0.14 does not expose a typed builder for `IFLA_BR_STP_STATE`
/// (it lives inside the nested `IFLA_LINKINFO` -> `IFLA_INFO_DATA` ->
/// `IFLA_BR_STP_STATE` attribute and the crate's bridge builder only
/// covers it at create-time, not as a post-create modification). The
/// portable kernel-supported alternative is the sysfs knob at
/// `/sys/class/net/<name>/bridge/stp_state`, which is what
/// `brctl stp <name> on|off` writes under the hood. We use the sysfs
/// path so the helper works on every kernel that has bridge support
/// without depending on an rtnetlink API surface that may move
/// between crate versions.
///
/// # Errors
///
/// Returns [`NetlinkError::NotFound`] if the bridge does not exist (no
/// `/sys/class/net/<name>/bridge` directory). Returns
/// [`NetlinkError::Io`] for any other write failure (permission
/// denied, the link exists but is not a bridge, etc.).
#[cfg(target_os = "linux")]
pub fn set_bridge_stp(name: &str, stp_on: bool) -> Result<(), NetlinkError> {
    let bridge_dir = format!("/sys/class/net/{name}/bridge");
    if !std::path::Path::new(&bridge_dir).exists() {
        return Err(NetlinkError::NotFound(name.to_string()));
    }
    let path = format!("{bridge_dir}/stp_state");
    let value = if stp_on { "1" } else { "0" };
    std::fs::write(&path, value)?;
    Ok(())
}

/// Non-Linux stub.
#[cfg(not(target_os = "linux"))]
pub fn set_bridge_stp(_name: &str, _stp_on: bool) -> Result<(), NetlinkError> {
    Err(NetlinkError::Netlink(
        "set_bridge_stp is only supported on Linux".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    // The helpers and tests in this module are Linux-only (they require
    // netlink + CAP_NET_ADMIN). Keep imports/fixtures gated so the lib
    // tests still compile on Windows/macOS cross-checks.
    #[cfg(target_os = "linux")]
    use super::*;

    /// Generate a short random-ish suffix for test interface names so
    /// parallel `cargo test` invocations don't collide. Bounded to 6
    /// chars so the full name (`zlb-` prefix + suffix) stays under the
    /// 15-char `IFNAMSIZ` limit.
    #[cfg(target_os = "linux")]
    fn rand_suffix() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        const CHARS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        // base36-ish, 6 chars
        let mut n = u64::from(nanos);
        let mut out = String::new();
        let base = CHARS.len() as u64;
        for _ in 0..6 {
            let idx = usize::try_from(n % base).unwrap_or(0);
            out.push(CHARS[idx] as char);
            n /= base;
        }
        out
    }

    /// Create a dummy interface with the given name (used as a stand-in
    /// for a host-side veth end in `bridge_add_link_membership`).
    #[cfg(target_os = "linux")]
    async fn create_dummy(name: &str) -> Result<(), NetlinkError> {
        let (connection, handle, _) = rtnetlink::new_connection()
            .map_err(|e| NetlinkError::Netlink(format!("new_connection failed: {e}")))?;
        tokio::spawn(connection);
        handle
            .link()
            .add()
            .dummy(name.to_string())
            .execute()
            .await
            .map_err(|e| NetlinkError::Netlink(format!("dummy create failed for {name}: {e}")))
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "requires CAP_NET_ADMIN; run manually or in privileged CI"]
    async fn bridge_create_idempotent() {
        let name = format!("zlb-{}", rand_suffix());
        assert!(name.len() <= 15, "interface name exceeds IFNAMSIZ: {name}");

        // First create.
        create_bridge(&name).await.expect("first create_bridge");
        assert!(
            std::path::Path::new(&format!("/sys/class/net/{name}")).exists(),
            "bridge {name} should exist after create"
        );

        // Second create on same name must be Ok.
        create_bridge(&name)
            .await
            .expect("second create_bridge should be idempotent");

        // Delete and confirm gone.
        delete_bridge(&name).await.expect("delete_bridge");
        assert!(
            !std::path::Path::new(&format!("/sys/class/net/{name}")).exists(),
            "bridge {name} should be gone after delete"
        );

        // Second delete on missing name must be Ok.
        delete_bridge(&name)
            .await
            .expect("second delete_bridge should be idempotent");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "requires CAP_NET_ADMIN; run manually or in privileged CI"]
    async fn bridge_add_link_membership() {
        let suffix = rand_suffix();
        let bridge = format!("zlb-{suffix}");
        let dummy = format!("zld-{suffix}");
        assert!(bridge.len() <= 15);
        assert!(dummy.len() <= 15);

        create_bridge(&bridge).await.expect("create_bridge");
        create_dummy(&dummy).await.expect("create_dummy");

        add_link_to_bridge(&dummy, &bridge)
            .await
            .expect("add_link_to_bridge");

        // The dummy's master/ifindex symlink should resolve to the
        // bridge's ifindex.
        let master_ifindex_path = format!("/sys/class/net/{dummy}/master/ifindex");
        let dummy_master_ifindex = std::fs::read_to_string(&master_ifindex_path)
            .expect("read dummy master ifindex")
            .trim()
            .parse::<u32>()
            .expect("parse dummy master ifindex");

        let bridge_ifindex = std::fs::read_to_string(format!("/sys/class/net/{bridge}/ifindex"))
            .expect("read bridge ifindex")
            .trim()
            .parse::<u32>()
            .expect("parse bridge ifindex");

        assert_eq!(
            dummy_master_ifindex, bridge_ifindex,
            "dummy's master ifindex should equal bridge's ifindex"
        );

        // Cleanup.
        delete_link_by_name(&dummy).await.expect("delete dummy");
        delete_bridge(&bridge).await.expect("delete bridge");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "requires CAP_NET_ADMIN; run manually or in privileged CI"]
    async fn bridge_stp_off() {
        let name = format!("zlb-{}", rand_suffix());
        assert!(name.len() <= 15);

        create_bridge(&name).await.expect("create_bridge");

        set_bridge_stp(&name, false).expect("set_bridge_stp off");
        let stp_state = std::fs::read_to_string(format!("/sys/class/net/{name}/bridge/stp_state"))
            .expect("read stp_state")
            .trim()
            .to_string();
        assert_eq!(
            stp_state, "0",
            "stp_state should be 0 after set_bridge_stp(false)"
        );

        // Cleanup.
        delete_bridge(&name).await.expect("delete_bridge");
    }
}
