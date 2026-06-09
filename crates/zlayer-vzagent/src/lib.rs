//! `zlayer-vzagent` — the in-guest PID1 agent for the macOS Apple-Virtualization
//! (VZ) Linux runtime, plus the cross-platform vsock wire protocol it speaks.
//!
//! On Apple Silicon macOS, `ZLayer` boots a real Linux guest through
//! `VZLinuxBootLoader` to run OCI Linux containers. The host side (in
//! `zlayer-agent`) talks to this tiny PID1 agent over **virtio-vsock**.
//!
//! This crate is split in two:
//!
//! * The [`proto`] module — the vsock wire protocol. It is fully
//!   cross-platform (no Linux-only dependencies) so the **host** crate can
//!   reuse the exact same encoder/decoder. It builds on macOS.
//! * The binary (`src/main.rs`) — the PID1 agent. Its real body is Linux-only
//!   and `#[cfg(target_os = "linux")]`-gated; on non-Linux it is a stub so the
//!   crate still compiles on the macOS host.

pub mod proto;
