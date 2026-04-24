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
//!
//! # `unsafe` policy
//!
//! This crate is the authoritative Windows HCN FFI boundary for `ZLayer`. It
//! exists specifically to wrap the unsafe `computenetwork.dll` entry points
//! behind safe, typed Rust APIs, so `unsafe` is allowed at the crate level
//! here even though the workspace-wide policy (`-W unsafe-code`) forbids it
//! everywhere else. Every `unsafe` block, `unsafe impl`, `unsafe fn`, and
//! `unsafe` method in this crate carries a `SAFETY:` comment explaining why
//! the required invariants hold — do not add a new `unsafe` site without one.

#![cfg(windows)]
#![allow(unsafe_code)]
// FFI-boundary lints: this crate's entire job is marshaling data into raw
// pointers for the HCN C ABI. We take `&mut x` / `&x` borrows and hand
// them to `Hcn*` entry points that take `*mut T` / `*const T`; we cast
// between related pointer shapes (e.g. the stable `*const c_void` alias
// in `handle.rs` vs the `*mut c_void` out-params HCN returns); and we
// navigate `GetAdaptersAddresses`'s variable-length IP_ADAPTER_ADDRESSES_LH
// linked list by casting from `*const u8` after byte-offset arithmetic.
// Each site is inside an `unsafe` block with a `SAFETY:` comment; the
// workspace-wide policy for these lints remains in force everywhere else.
#![allow(
    clippy::borrow_as_ptr,
    clippy::ptr_as_ptr,
    clippy::ptr_cast_constness,
    clippy::cast_ptr_alignment,
    clippy::cast_possible_wrap
)]

pub mod adapter;
pub mod attach;
pub mod endpoint;
pub mod error;
pub mod handle;
pub mod namespace;
pub mod network;
pub mod schema;
