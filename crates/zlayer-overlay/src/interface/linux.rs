//! Linux implementation of [`InterfaceOps`] — thin wrapper around the
//! existing [`crate::netlink`] helpers.
//!
//! Preserves the RTNETLINK-based behavior byte-for-byte; this module is
//! only an adapter that plugs the Linux path into the platform-agnostic
//! [`InterfaceOps`] trait.

use std::net::IpAddr;

use async_trait::async_trait;

use crate::interface::InterfaceOps;
use crate::netlink::NetlinkError;
use crate::OverlayError;

fn netlink_err_to_overlay(err: NetlinkError) -> OverlayError {
    match err {
        NetlinkError::Io(io) => OverlayError::Io(io),
        NetlinkError::NotFound(name) => OverlayError::InterfaceNotFound(name),
        NetlinkError::Netlink(msg) => OverlayError::NetworkConfig(msg),
    }
}

pub(crate) struct LinuxNetlinkOps;

impl LinuxNetlinkOps {
    pub(crate) fn new() -> Self {
        Self
    }
}

#[async_trait]
impl InterfaceOps for LinuxNetlinkOps {
    async fn link_exists(&self, name: &str) -> Result<bool, OverlayError> {
        crate::netlink::link_exists(name)
            .await
            .map_err(netlink_err_to_overlay)
    }

    async fn delete_link(&self, name: &str) -> Result<(), OverlayError> {
        crate::netlink::delete_link_by_name(name)
            .await
            .map_err(netlink_err_to_overlay)
    }

    async fn set_link_up(&self, name: &str) -> Result<(), OverlayError> {
        crate::netlink::set_link_up_by_name(name)
            .await
            .map_err(netlink_err_to_overlay)
    }

    async fn add_address(
        &self,
        name: &str,
        addr: IpAddr,
        prefix_len: u8,
    ) -> Result<(), OverlayError> {
        crate::netlink::add_address_to_link(name, addr, prefix_len)
            .await
            .map_err(netlink_err_to_overlay)
    }

    async fn add_route_via_dev(
        &self,
        dest: IpAddr,
        prefix_len: u8,
        name: &str,
    ) -> Result<(), OverlayError> {
        crate::netlink::add_route_via_dev(dest, prefix_len, name)
            .await
            .map_err(netlink_err_to_overlay)
    }
}
