//! Translators from Docker Engine API wire types (`super::types`) to
//! `ZLayer`'s native `ServiceSpec`/`ResourcesSpec` and related shapes.
//!
//! Wired into `super::containers::create_container` in sub-task 2.4.5 — every
//! module here is now consumed by `build_create_request`, so the previous
//! crate-level `#[allow(dead_code)]` shim has been removed.

pub mod caps;
pub mod healthcheck;
pub mod mounts;
pub mod ports;
pub mod resources;
pub mod restart;
