//! Fallback [`InterfaceOps`] implementation for targets that do not yet
//! have a real backend.
//!
//! Every method returns [`OverlayError::NetworkConfig`] with a message
//! indicating the operation is not implemented on the current
//! platform. Windows support will replace this in Phase D3.

use std::net::IpAddr;

use async_trait::async_trait;

use crate::interface::InterfaceOps;
use crate::OverlayError;

const UNSUPPORTED_MSG: &str = "interface ops not implemented on this OS";

pub(crate) struct StubOps;

#[async_trait]
impl InterfaceOps for StubOps {
    async fn link_exists(&self, _name: &str) -> Result<bool, OverlayError> {
        Err(OverlayError::NetworkConfig(UNSUPPORTED_MSG.to_string()))
    }

    async fn delete_link(&self, _name: &str) -> Result<(), OverlayError> {
        Err(OverlayError::NetworkConfig(UNSUPPORTED_MSG.to_string()))
    }

    async fn set_link_up(&self, _name: &str) -> Result<(), OverlayError> {
        Err(OverlayError::NetworkConfig(UNSUPPORTED_MSG.to_string()))
    }

    async fn add_address(
        &self,
        _name: &str,
        _addr: IpAddr,
        _prefix_len: u8,
    ) -> Result<(), OverlayError> {
        Err(OverlayError::NetworkConfig(UNSUPPORTED_MSG.to_string()))
    }

    async fn add_route_via_dev(
        &self,
        _dest: IpAddr,
        _prefix_len: u8,
        _name: &str,
    ) -> Result<(), OverlayError> {
        Err(OverlayError::NetworkConfig(UNSUPPORTED_MSG.to_string()))
    }
}
