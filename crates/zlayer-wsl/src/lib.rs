//! WSL2 backend integration for `ZLayer` on Windows.
//!
//! This crate provides automatic WSL2 management so `ZLayer` can run
//! on Windows with a Docker Desktop-like experience. A thin Windows
//! CLI binary proxies commands to a full `ZLayer` daemon running inside
//! a dedicated WSL2 distribution.
//!
//! On non-Windows platforms, all functions return errors or no-op values.

pub mod daemon;
pub mod detect;
pub mod distro;
pub mod errors;
pub mod paths;
pub mod setup;
