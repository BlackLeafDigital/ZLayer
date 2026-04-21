//! Windows-specific container primitives (HCS, WCIFS, HNS).
//!
//! This subtree groups the Win32/HCS glue that the agent needs to materialize
//! and run Windows containers. It is only compiled on Windows targets.
#![cfg(target_os = "windows")]

pub mod layer;
pub mod scratch;
pub mod unpacker;
pub mod wclayer;
