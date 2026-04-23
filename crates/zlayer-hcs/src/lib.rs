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
//!
//! # `unsafe` policy
//!
//! This crate is the authoritative Windows HCS FFI boundary for `ZLayer`. It
//! exists specifically to wrap the unsafe `vmcompute.dll` entry points behind
//! safe, typed Rust APIs, so `unsafe` is allowed at the crate level here even
//! though the workspace-wide policy (`-W unsafe-code`) forbids it everywhere
//! else. Every `unsafe` block, `unsafe impl`, `unsafe fn`, and `unsafe` method
//! in this crate carries a `SAFETY:` comment explaining why the required
//! invariants hold — do not add a new `unsafe` site without one.

#![cfg(windows)]
#![allow(unsafe_code)]

pub mod enumerate;
pub mod error;
pub mod events;
pub mod handle;
pub mod operation;
pub mod process;
pub mod schema;
pub mod stats;
pub mod system;

pub use handle::SendHandle;
