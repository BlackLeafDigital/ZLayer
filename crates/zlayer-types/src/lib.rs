//! Shared wire types for the `ZLayer` platform.
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

/// Serde helpers to (de)serialize an [`ImageReference`] as its OCI-spec
/// canonical string form (`[host[:port]/]name[:tag][@digest]`) instead of
/// the default struct shape `{registry, repository, tag, digest}`.
///
/// Use with `#[serde(with = "zlayer_types::image_ref_serde")]` on a field
/// of type `ImageReference`. For optional fields use `image_ref_serde::option`.
pub mod image_ref_serde {
    use super::ImageReference;
    use serde::{Deserialize, Deserializer, Serializer};
    use std::str::FromStr;

    /// # Errors
    ///
    /// Returns the serializer's error if writing the string form fails.
    pub fn serialize<S: Serializer>(r: &ImageReference, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&r.to_string())
    }

    /// # Errors
    ///
    /// Returns a deserialization error if the input is not a valid OCI image reference string.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<ImageReference, D::Error> {
        let s = String::deserialize(d)?;
        ImageReference::from_str(&s).map_err(serde::de::Error::custom)
    }

    pub mod option {
        use super::ImageReference;
        use serde::{Deserialize, Deserializer, Serializer};
        use std::str::FromStr;

        /// # Errors
        ///
        /// Returns the serializer's error if writing the string form fails.
        pub fn serialize<S: Serializer>(
            r: &Option<ImageReference>,
            s: S,
        ) -> Result<S::Ok, S::Error> {
            match r {
                Some(r) => s.serialize_str(&r.to_string()),
                None => s.serialize_none(),
            }
        }

        /// # Errors
        ///
        /// Returns a deserialization error if the input is not a valid OCI image reference string.
        pub fn deserialize<'de, D: Deserializer<'de>>(
            d: D,
        ) -> Result<Option<ImageReference>, D::Error> {
            let s: Option<String> = Option::deserialize(d)?;
            match s {
                Some(s) => ImageReference::from_str(&s)
                    .map(Some)
                    .map_err(serde::de::Error::custom),
                None => Ok(None),
            }
        }
    }
}

/// Wire-type modules. Each maps to one logical area; downstream crates
/// import via `pub use zlayer_types::<area>::...`.
pub mod api;
pub mod auth;
pub mod client;
pub mod cluster;
pub mod jwt;
pub mod overlay;
pub mod scratch;
pub mod secrets;
pub mod spec;
pub mod storage;

pub use scratch::{Scratch, ScratchFile};
