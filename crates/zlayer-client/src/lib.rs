//! HTTP client library for the `ZLayer` daemon's REST API.
//!
//! This crate exposes [`DaemonClient`], a typed HTTP client that talks to a
//! running `zlayer serve` daemon, plus the on-disk [`session`] machinery used
//! to attach `Authorization: Bearer <token>` headers.
//!
//! On Unix platforms the transport is HTTP over a Unix-domain socket
//! (platform-dependent path; see `default_socket_path`). On Windows the
//! transport is HTTP over TCP on `127.0.0.1:3669`.
//!
//! It was extracted from `bin/zlayer` so library crates (`zlayer-docker`,
//! `zlayer-py`, future language SDKs) can embed the same daemon client
//! without depending on the CLI binary.

pub mod session;

mod daemon_client;

pub use daemon_client::{default_socket_path, BuildHandle, BuildSpec, DaemonClient};
