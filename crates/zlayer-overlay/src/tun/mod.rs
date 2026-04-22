//! Platform-abstracted TUN device traits.
//!
//! Linux/macOS currently delegate TUN handling to boringtun's
//! [`boringtun::device::DeviceHandle`] (see [`crate::transport`]). Windows
//! uses a Wintun adapter explicitly — the `device` feature of boringtun
//! does not support Wintun, so we drive the Windows side ourselves via
//! this module's platform implementation.
//!
//! This is an internal seam only; the public transport API in
//! [`crate::transport`] remains identical across all three targets.

#![allow(dead_code)]

#[cfg(windows)]
pub(crate) mod windows;

#[cfg(windows)]
pub(crate) use self::windows::WindowsTun;
