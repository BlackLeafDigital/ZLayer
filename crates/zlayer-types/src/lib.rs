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

/// Image reference that preserves the user's RAW input string alongside
/// the parsed canonical [`ImageReference`].
///
/// `ImageReference` (from `oci_client`) normalizes Docker Hub defaults at
/// parse time — `nginx` becomes `docker.io/library/nginx`. That's correct
/// for resolution, but destroys the ability to detect "the user wrote a
/// bare name and we should look for it locally first."
///
/// `ImageRef` holds both:
/// - `parsed`: the canonical [`ImageReference`] for normalized comparisons,
///   registry calls, and tag/digest extraction.
/// - `original`: the exact bytes the user (or wire format) gave us.
///
/// Equality and hashing use the canonical `parsed` form, so two `ImageRef`s
/// resolving to the same image compare equal even if one was qualified.
/// `Display` / `Serialize` use the original, so roundtripping preserves
/// what the user typed.
#[derive(Debug, Clone)]
pub struct ImageRef {
    parsed: ImageReference,
    original: String,
}

impl ImageRef {
    /// Wrap an already-parsed [`ImageReference`]. The `original` field is
    /// set to the canonical string form, so [`Self::is_unqualified`] on the
    /// result will return `false` (the canonical form always has a host).
    #[must_use]
    pub fn from_parsed(parsed: ImageReference) -> Self {
        let original = parsed.to_string();
        Self { parsed, original }
    }

    /// Access the parsed canonical [`ImageReference`].
    #[must_use]
    pub fn parsed(&self) -> &ImageReference {
        &self.parsed
    }

    /// Access the user's original input string verbatim.
    #[must_use]
    pub fn original(&self) -> &str {
        &self.original
    }

    /// Returns true when the user's original string did NOT include a registry
    /// host (no `host[:port]/` prefix). Detection rule: split on the FIRST `/`;
    /// if there's no `/` at all, or the segment before the first `/` contains
    /// neither `.` nor `:` and is not `localhost`, treat as unqualified.
    ///
    /// Examples:
    /// - `nginx`                              -> unqualified
    /// - `nginx:latest`                       -> unqualified
    /// - `library/nginx`                      -> unqualified (Docker namespace, no host)
    /// - `foo/bar`                            -> unqualified
    /// - `docker.io/library/nginx`            -> qualified (host has `.`)
    /// - `ghcr.io/foo/bar`                    -> qualified (host has `.`)
    /// - `localhost/foo`                      -> qualified (literal `localhost`)
    /// - `localhost:5000/foo`                 -> qualified (`:` port)
    /// - `registry:5000/foo`                  -> qualified (`:` port)
    #[must_use]
    pub fn is_unqualified(&self) -> bool {
        image_str_is_unqualified(&self.original)
    }
}

/// Standalone form of [`ImageRef::is_unqualified`] operating on a raw string.
///
/// Useful at boundary layers (e.g. the registry client) that receive the
/// user-original string via [`ImageRef::to_string`] / `Display` and need to
/// decide whether to fall back to a remote pull without round-tripping
/// through `ImageRef`.
///
/// Detection rule matches [`ImageRef::is_unqualified`]: split on the first
/// `/`; if there's no `/` at all, or the segment before the first `/`
/// contains neither `.` nor `:` and is not `localhost`, the reference is
/// considered unqualified.
#[must_use]
pub fn image_str_is_unqualified(s: &str) -> bool {
    let without_digest = match s.split_once('@') {
        Some((head, _)) => head,
        None => s,
    };
    let Some((head, _rest)) = without_digest.split_once('/') else {
        return true;
    };
    if head == "localhost" {
        return false;
    }
    if head.contains('.') || head.contains(':') {
        return false;
    }
    true
}

impl std::str::FromStr for ImageRef {
    type Err = <ImageReference as std::str::FromStr>::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parsed = ImageReference::from_str(s)?;
        Ok(Self {
            parsed,
            original: s.to_string(),
        })
    }
}

impl std::ops::Deref for ImageRef {
    type Target = ImageReference;

    fn deref(&self) -> &Self::Target {
        &self.parsed
    }
}

impl std::fmt::Display for ImageRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.original)
    }
}

impl PartialEq for ImageRef {
    fn eq(&self, other: &Self) -> bool {
        self.parsed == other.parsed
    }
}

impl Eq for ImageRef {}

impl std::hash::Hash for ImageRef {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // `ImageReference` doesn't implement `Hash`, so hash its canonical
        // string form. This stays consistent with `PartialEq`, which
        // compares `parsed` directly.
        self.parsed.to_string().hash(state);
    }
}

impl serde::Serialize for ImageRef {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.original)
    }
}

impl<'de> serde::Deserialize<'de> for ImageRef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use std::str::FromStr;
        let s = String::deserialize(d)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}

/// Wire-type modules. Each maps to one logical area; downstream crates
/// import via `pub use zlayer_types::<area>::...`.
pub mod api;
pub mod auth;
pub mod builder;
pub mod client;
pub mod cluster;
pub mod jwt;
pub mod overlay;
pub mod scratch;
pub mod secrets;
pub mod spec;
pub mod storage;

pub use scratch::{Scratch, ScratchFile};

#[cfg(test)]
mod image_ref_tests {
    use super::ImageRef;
    use std::str::FromStr;

    fn parse(s: &str) -> ImageRef {
        ImageRef::from_str(s).unwrap_or_else(|e| panic!("failed to parse {s:?}: {e}"))
    }

    #[test]
    fn unqualified_bare_name() {
        assert!(parse("nginx").is_unqualified());
    }

    #[test]
    fn unqualified_bare_name_with_tag() {
        assert!(parse("nginx:latest").is_unqualified());
    }

    #[test]
    fn unqualified_library_namespace() {
        assert!(parse("library/nginx").is_unqualified());
    }

    #[test]
    fn unqualified_user_namespace_with_tag() {
        assert!(parse("foo/bar:1.0").is_unqualified());
    }

    #[test]
    fn qualified_docker_io() {
        assert!(!parse("docker.io/library/nginx").is_unqualified());
    }

    #[test]
    fn qualified_ghcr() {
        assert!(!parse("ghcr.io/foo/bar:1.2").is_unqualified());
    }

    #[test]
    fn qualified_localhost() {
        assert!(!parse("localhost/foo").is_unqualified());
    }

    #[test]
    fn qualified_localhost_port() {
        assert!(!parse("localhost:5000/foo").is_unqualified());
    }

    #[test]
    fn qualified_registry_port() {
        assert!(!parse("registry:5000/foo").is_unqualified());
    }

    #[test]
    fn unqualified_with_digest_tail() {
        // Bare name + digest: the host detection must strip the `@sha256:...`
        // tail before scanning for `/`. There is no `/` left, so unqualified.
        let s = "foo@sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert!(parse(s).is_unqualified());
    }

    #[test]
    fn display_preserves_user_input() {
        // `nginx:latest` would canonicalize to `docker.io/library/nginx:latest`;
        // Display must return the original verbatim.
        let r = ImageRef::from_str("nginx:latest").unwrap();
        assert_eq!(r.to_string(), "nginx:latest");
    }

    #[test]
    fn serde_json_roundtrip_preserves_original() {
        let r = ImageRef::from_str("zarcrunner-executor:latest").unwrap();
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, "\"zarcrunner-executor:latest\"");
        let back: ImageRef = serde_json::from_str(&json).unwrap();
        assert_eq!(back.original(), "zarcrunner-executor:latest");
    }

    #[test]
    fn equality_uses_canonical_form() {
        // `nginx:latest` and `docker.io/library/nginx:latest` canonicalize
        // to the same `ImageReference`, so the wrappers must compare equal
        // even though their `original` strings differ.
        let bare = ImageRef::from_str("nginx:latest").unwrap();
        let qualified = ImageRef::from_str("docker.io/library/nginx:latest").unwrap();
        assert_eq!(bare, qualified);
    }
}
