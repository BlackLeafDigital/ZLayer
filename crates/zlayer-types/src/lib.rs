//! Shared wire types for the ZLayer platform.
//!
//! This crate is the SDK-facing types crate: API DTOs, OCI image
//! references, and other serde-friendly wire shapes consumed by both
//! the daemon and clients. It is intentionally lightweight — no axum,
//! no tokio, no reqwest. Heavier server-side abstractions live in
//! `zlayer-api`, `zlayer-core`, and friends.

/// Canonical OCI image reference.
///
/// Re-export of [`oci_client::Reference`] (which itself re-exports
/// `oci_spec::distribution::Reference`). Use this as the wire type for
/// any image reference — the OCI spec grammar
/// `[host[:port]/]name[:tag][@digest]`, with built-in normalization
/// for Docker Hub defaults.
pub use oci_client::Reference as ImageReference;
