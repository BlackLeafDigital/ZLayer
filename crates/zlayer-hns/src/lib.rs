//! Safe Rust wrapper for the Windows Host Compute Network Service (HCN v2).
//!
//! HCN is the networking half of the Windows container stack — companion to
//! the Host Compute Service (HCS). The underlying surface lives in
//! `computenetwork.dll`. This crate wraps those raw calls with RAII handle
//! types, async-ready helpers around the synchronous HCN API, JSON schema
//! types matching `hcsshim/hcn` v2, and high-level `NetAttacher`-friendly
//! abstractions for attaching containers to networks via HCN namespaces.
//!
//! The crate is Windows-only. On every other platform it compiles to an
//! empty stub so callers can depend on it unconditionally.

#![cfg(windows)]

pub mod adapter;
pub mod attach;
pub mod endpoint;
pub mod error;
pub mod handle;
pub mod namespace;
pub mod network;
pub mod schema;
