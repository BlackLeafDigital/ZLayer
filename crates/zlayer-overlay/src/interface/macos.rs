//! macOS implementation of [`InterfaceOps`].
//!
//! Shells out to `ifconfig` and `route` — the same commands (byte for
//! byte) that `transport.rs` used to issue inline. This module just
//! moves those invocations behind the [`InterfaceOps`] trait so that
//! `transport.rs` can be platform-agnostic.
//!
//! The BSD-flavored userspace on macOS does not expose a clean
//! programmatic equivalent, so `ifconfig` / `route` remain the standard
//! approach. All commands match what `wireguard-go` / `wg-quick` use on
//! macOS.

use std::net::IpAddr;

use async_trait::async_trait;
use tokio::process::Command;

use crate::interface::InterfaceOps;
use crate::OverlayError;

/// Compute the IPv4 netmask (dotted-quad) for a given prefix length.
fn v4_netmask(prefix_len: u8) -> String {
    // Saturate at 32 — callers should already pass a valid v4 prefix.
    let p = std::cmp::min(prefix_len, 32);
    let mask: u32 = if p == 0 { 0 } else { u32::MAX << (32 - p) };
    let a = (mask >> 24) & 0xff;
    let b = (mask >> 16) & 0xff;
    let c = (mask >> 8) & 0xff;
    let d = mask & 0xff;
    format!("{a}.{b}.{c}.{d}")
}

pub(crate) struct MacIfconfigOps;

impl MacIfconfigOps {
    pub(crate) fn new() -> Self {
        Self
    }
}

#[async_trait]
impl InterfaceOps for MacIfconfigOps {
    async fn link_exists(&self, name: &str) -> Result<bool, OverlayError> {
        // `ifconfig <name>` exits 0 if the interface exists, non-zero otherwise.
        let output = Command::new("ifconfig")
            .arg(name)
            .output()
            .await
            .map_err(OverlayError::Io)?;
        Ok(output.status.success())
    }

    async fn delete_link(&self, name: &str) -> Result<(), OverlayError> {
        // `ifconfig <name> destroy` — ignore "does not exist" errors for
        // idempotency, matching the Linux netlink path.
        let output = Command::new("ifconfig")
            .args([name, "destroy"])
            .output()
            .await
            .map_err(OverlayError::Io)?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("does not exist") || stderr.contains("no such") {
            return Ok(());
        }
        Err(OverlayError::NetworkConfig(format!(
            "failed to destroy interface {name}: {stderr}"
        )))
    }

    async fn set_link_up(&self, name: &str) -> Result<(), OverlayError> {
        // On macOS, `add_address` already passes `up` to ifconfig, so
        // this is normally a no-op. Provide an explicit form for
        // correctness / parity with the Linux path.
        let output = Command::new("ifconfig")
            .args([name, "up"])
            .output()
            .await
            .map_err(OverlayError::Io)?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(OverlayError::NetworkConfig(format!(
                "failed to bring interface up: {stderr}"
            )));
        }
        Ok(())
    }

    async fn add_address(
        &self,
        name: &str,
        addr: IpAddr,
        prefix_len: u8,
    ) -> Result<(), OverlayError> {
        // Matches transport.rs:316-348 exactly: point-to-point utun
        // config with identical local/remote IPs.
        let ip_str = addr.to_string();
        let output = match addr {
            IpAddr::V4(_) => {
                let netmask = v4_netmask(prefix_len);
                Command::new("ifconfig")
                    .args([name, "inet", &ip_str, &ip_str, "netmask", &netmask, "up"])
                    .output()
                    .await
                    .map_err(OverlayError::Io)?
            }
            IpAddr::V6(_) => {
                let prefixlen = prefix_len.to_string();
                Command::new("ifconfig")
                    .args([name, "inet6", &ip_str, "prefixlen", &prefixlen, "up"])
                    .output()
                    .await
                    .map_err(OverlayError::Io)?
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(OverlayError::NetworkConfig(format!(
                "Failed to configure interface: {stderr}"
            )));
        }
        Ok(())
    }

    async fn add_route_via_dev(
        &self,
        dest: IpAddr,
        prefix_len: u8,
        name: &str,
    ) -> Result<(), OverlayError> {
        // Matches transport.rs:357-393 exactly: add a link-scope route
        // to the overlay subnet via the utun interface.
        let network_cidr = format!("{dest}/{prefix_len}");
        let output = match dest {
            IpAddr::V4(_) => Command::new("route")
                .args(["-n", "add", "-net", &network_cidr, "-interface", name])
                .output()
                .await
                .map_err(OverlayError::Io)?,
            IpAddr::V6(_) => Command::new("route")
                .args(["-n", "add", "-inet6", &network_cidr, "-interface", name])
                .output()
                .await
                .map_err(OverlayError::Io)?,
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Ignore "already in table" — idempotent, matches the
            // original inline implementation.
            if !stderr.contains("already in table") {
                return Err(OverlayError::NetworkConfig(format!(
                    "Failed to add route: {stderr}"
                )));
            }
        }
        Ok(())
    }
}
