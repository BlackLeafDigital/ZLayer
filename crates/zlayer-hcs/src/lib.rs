//! Safe Rust wrapper for the Windows Host Compute Service (HCS).
//!
//! HCS is the OS-level API powering Windows containers and Hyper-V-isolated
//! workloads; the underlying surface lives in `vmcompute.dll`. This crate
//! wraps those raw calls with RAII handle types, async operation helpers that
//! translate HCS's native completion-callback model into Rust futures, and
//! JSON schema types that match the current hcsshim schema v2.
//!
//! The crate is Windows-only. On every other platform it compiles to an
//! empty stub so callers can depend on it unconditionally.

#![cfg(windows)]

pub mod enumerate;
pub mod error;
pub mod events;
pub mod handle;
pub mod operation;
pub mod process;
pub mod schema;
pub mod stats;
pub mod system;
