//! Windows-specific build-time helpers.
//!
//! This module hosts validation and tooling that only applies when the target
//! OS is Windows. The Linux / macOS builder paths bypass it entirely.
//!
//! # Submodules
//!
//! - [`deps`] — static checks on `RUN` instructions that catch
//!   `choco install` / `winget install` usage on `nanoserver` base images,
//!   which lack `PowerShell` and therefore cannot run either package manager.

pub mod deps;
