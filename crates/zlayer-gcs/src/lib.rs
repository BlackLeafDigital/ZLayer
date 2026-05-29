//! GCS (Guest Compute Service) bridge for Windows Hyper-V utility VMs.
//!
//! Host-side library that connects to a running UVM's in-guest GCS over an
//! hvsock (Hyper-V virtual socket), negotiates the GCS protocol, and
//! dispatches RPCs (`CreateContainer`, `ExecuteProcess`, `ModifySettings`, ...).
//!
//! Mirrors hcsshim's `internal/gcs/{guestconnection.go, container.go,
//! prot/protocol.go}` Go reference implementation.
//!
//! All FFI sits behind `#[cfg(target_os = "windows")]` — the crate compiles
//! cleanly on non-Windows hosts (with the public types stubbed) so the rest
//! of the workspace stays buildable on Linux/`macOS` dev boxes.

#![cfg_attr(not(target_os = "windows"), allow(dead_code, unused_imports))]
#![deny(rust_2018_idioms)]
#![warn(clippy::pedantic, clippy::nursery)]
#![allow(
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

pub mod bridge;
pub mod error;
pub mod frame;
pub mod protocol;
pub mod transport;

pub use error::{GcsError, GcsResult};

#[cfg(test)]
mod tests {
    #[test]
    fn crate_exports_compile() {
        // Just ensures every public symbol typechecks.
        let _: super::GcsError = std::io::Error::other("x").into();
    }
}
