//! Rust netlink helpers that replace shell-outs to `ip` for overlay TUN
//! transport setup.
//!
//! This module is the Linux-only counterpart to the shell-outs previously
//! used by [`crate::transport`] to configure the boringtun-created TUN
//! device:
//!
//! - `ip link show <iface>`           → [`link_exists`]
//! - `ip link delete <iface>`         → [`delete_link_by_name`]
//! - `ip addr add <cidr> dev <iface>` → [`add_address_to_link`]
//! - `ip link set dev <iface> up`     → [`set_link_up_by_name`]
//! - `ip route add <net> dev <iface>` → [`add_route_via_dev`]
//!
//! All helpers talk RTNETLINK directly via the `rtnetlink` crate (async,
//! tokio-backed). The module is compiled on Linux only — the macOS branch
//! of `transport.rs` uses a completely different path (utun + ifconfig +
//! route) and never touches these helpers.

use std::net::IpAddr;

use thiserror::Error;

/// Errors returned by the netlink helpers in this module.
#[derive(Debug, Error)]
pub enum NetlinkError {
    /// Failed to open or access a file.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// The requested link was not found in the current network namespace.
    #[error("link '{0}' not found")]
    NotFound(String),

    /// A netlink operation failed.
    #[error("netlink operation failed: {0}")]
    Netlink(String),
}

/// Returns `Ok(true)` if the named link exists in the current netns,
/// `Ok(false)` if it does not, and an error for any other RTNETLINK
/// failure (permission denied, etc.).
///
/// Replaces the shell-out:
///   ip link show `<name>`
///
/// Used by the stale-interface cleanup in
/// [`crate::transport::OverlayTransport::create_interface`] to detect
/// a leaked TUN device from a previously crashed `boringtun` instance.
///
/// # Errors
///
/// Returns [`NetlinkError::Netlink`] if the RTNETLINK connection fails
/// to initialize or the lookup returns an error other than
/// `ENODEV` / "No such device" (which are mapped to `Ok(false)`).
pub async fn link_exists(name: &str) -> Result<bool, NetlinkError> {
    use futures_util::stream::TryStreamExt;

    let (connection, handle, _) = rtnetlink::new_connection()
        .map_err(|e| NetlinkError::Netlink(format!("new_connection failed: {e}")))?;
    tokio::spawn(connection);

    let lookup = handle
        .link()
        .get()
        .match_name(name.to_string())
        .execute()
        .try_next()
        .await;

    match lookup {
        Ok(Some(_)) => Ok(true),
        Ok(None) => Ok(false),
        Err(rtnetlink::Error::NetlinkError(err)) => {
            let msg = err.to_string();
            // libc::ENODEV == 19 on Linux. Avoid a libc dep by hard-coding.
            let is_enodev = err.code.is_some_and(|c| c.get().unsigned_abs() == 19);
            if is_enodev || msg.contains("No such device") {
                Ok(false)
            } else {
                Err(NetlinkError::Netlink(format!(
                    "link lookup failed for {name}: {msg}"
                )))
            }
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("No such device") {
                Ok(false)
            } else {
                Err(NetlinkError::Netlink(format!(
                    "link lookup failed for {name}: {msg}"
                )))
            }
        }
    }
}

/// Delete the link by name. Idempotent: returns `Ok(())` if the link
/// does not exist.
///
/// Replaces the shell-out:
///   ip link delete `<name>`
///
/// Used by the stale-interface cleanup in
/// [`crate::transport::OverlayTransport::create_interface`].
///
/// # Errors
///
/// Returns [`NetlinkError::Netlink`] if RTNETLINK reports a failure
/// other than `ENODEV` / "No such device" (which are treated as
/// success so this is safe to call unconditionally).
pub async fn delete_link_by_name(name: &str) -> Result<(), NetlinkError> {
    use futures_util::stream::TryStreamExt;

    let (connection, handle, _) = rtnetlink::new_connection()
        .map_err(|e| NetlinkError::Netlink(format!("new_connection failed: {e}")))?;
    tokio::spawn(connection);

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
            let msg = err.to_string();
            // libc::ENODEV == 19 on Linux. Avoid a libc dep by hard-coding.
            let is_enodev = err.code.is_some_and(|c| c.get().unsigned_abs() == 19);
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

/// Set the link identified by `name` to the "up" administrative state.
///
/// Replaces the shell-out:
///   ip link set dev `<name>` up
///
/// # Errors
///
/// Returns [`NetlinkError::NotFound`] if no link with the given name
/// exists in the current netns. Returns [`NetlinkError::Netlink`] for
/// any other RTNETLINK failure (permission denied, etc.).
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

/// Add an IP address (v4 or v6) to the link identified by `name` in
/// the current network namespace.
///
/// Replaces the shell-out:
///   ip addr add `<addr>/<prefix_len>` dev `<name>`
///
/// `addr` may be v4 or v6. `prefix_len` is the CIDR prefix length
/// (24 for a `/24`, 64 for a `/64`, etc.).
///
/// # Errors
///
/// Returns [`NetlinkError::NotFound`] if the link is missing. Returns
/// [`NetlinkError::Netlink`] for any other rtnetlink failure. EEXIST
/// is NOT treated as success here — the caller (transport.rs) already
/// swallows duplicate-address errors at the call site to preserve the
/// original shell-out's idempotency.
pub async fn add_address_to_link(
    name: &str,
    addr: IpAddr,
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

/// Add a route to a subnet via a device name, with link scope
/// (direct, no gateway).
///
/// Replaces the shell-outs:
///   ip route add `<dest>/<prefix_len>` dev `<dev_name>`
///   ip -6 route add `<dest>/<prefix_len>` dev `<dev_name>`
///
/// Uses plain `add` semantics (not replace) to preserve the original
/// shell-out behavior — the caller is expected to swallow EEXIST at
/// the call site for idempotency.
///
/// The route is installed with link scope (direct-via-dev, no
/// gateway) which is the correct form for a point-to-point TUN
/// interface where the overlay subnet is reachable directly.
///
/// # Errors
///
/// Returns [`NetlinkError::NotFound`] if `dev_name` does not exist in
/// the current netns. Returns [`NetlinkError::Netlink`] for any other
/// RTNETLINK failure, including EEXIST (caller swallows it).
pub async fn add_route_via_dev(
    dest: IpAddr,
    prefix_len: u8,
    dev_name: &str,
) -> Result<(), NetlinkError> {
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

    match dest {
        IpAddr::V4(d) => handle
            .route()
            .add()
            .v4()
            .destination_prefix(d, prefix_len)
            .output_interface(oif_idx)
            .scope(RouteScope::Link)
            .execute()
            .await
            .map_err(|e| {
                NetlinkError::Netlink(format!(
                    "route add v4 {d}/{prefix_len} dev {dev_name} failed: {e}"
                ))
            }),
        IpAddr::V6(d) => handle
            .route()
            .add()
            .v6()
            .destination_prefix(d, prefix_len)
            .output_interface(oif_idx)
            .scope(RouteScope::Link)
            .execute()
            .await
            .map_err(|e| {
                NetlinkError::Netlink(format!(
                    "route add v6 {d}/{prefix_len} dev {dev_name} failed: {e}"
                ))
            }),
    }
}
