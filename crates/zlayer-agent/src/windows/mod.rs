//! Windows-specific container primitives (HCS, WCIFS, HNS).
//!
//! This subtree groups the Win32/HCS glue that the agent needs to materialize
//! and run Windows containers. It is only compiled on Windows targets.
//!
//! `#[cfg(target_os = "windows")]` is applied by the parent `lib.rs` module
//! declaration (`#[cfg(target_os = "windows")] pub mod windows;`), so it is
//! not repeated here.
//!
//! Every `unsafe` block in this subtree (and the direct HCS/HNS/HCN and
//! Win32 FFI call sites) has a `SAFETY:` comment; `unsafe_code` and the
//! companion pointer-family lints are allowed here for the same reasons
//! they are allowed in `zlayer-hcs` / `zlayer-hns`.
#![allow(unsafe_code, clippy::borrow_as_ptr)]

pub mod backuptar;
pub mod layer;
pub mod scratch;
pub mod timezone;
pub mod unpacker;
pub mod uvm;
pub mod wclayer;
