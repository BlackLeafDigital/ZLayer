//! HTTP client library for the `ZLayer` daemon's REST API.
//!
//! This crate exposes [`DaemonClient`], a typed HTTP-over-Unix-socket client
//! that talks to a running `zlayer serve` daemon, plus the on-disk
//! [`session`] machinery used to attach `Authorization: Bearer <token>`
//! headers.
//!
//! It was extracted from `bin/zlayer` so library crates (`zlayer-docker`,
//! `zlayer-py`, future language SDKs) can embed the same daemon client
//! without depending on the CLI binary.

pub mod session;

#[cfg(unix)]
mod daemon_client;

#[cfg(unix)]
pub use daemon_client::{default_socket_path, BuildHandle, BuildSpec, DaemonClient};
