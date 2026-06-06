//! `ZLayer` Registry - OCI image pulling and caching
//!
//! This module provides OCI distribution client functionality with local blob caching.
//!
//! ## Features
//!
//! - `persistent` - Enable persistent disk-based blob cache
//! - `s3` - Enable S3-compatible storage backend
//! - `local` - Enable local OCI registry for storing built images

pub mod cache;
pub mod cache_config;
pub mod client;
pub mod error;
pub mod image_config;
pub mod pack;
pub mod unpack;
pub mod wasm;

#[cfg(feature = "persistent")]
pub mod persistent_cache;

#[cfg(feature = "s3")]
pub mod s3_cache;

#[cfg(feature = "local")]
pub mod local_registry;

#[cfg(feature = "local")]
pub mod oci_export;

// `wasm_export` only depends on `cache::compute_digest` and the always-on
// `wasm` module, so it is available unconditionally — needed by
// `zlayer-builder` to export WASM OCI artifacts without pulling in the
// `local` feature's local-registry machinery.
pub mod wasm_export;

pub use cache::*;
pub use cache_config::{default_registry_from_env, s3_cache_from_env, CacheType};
pub use client::*;
pub use error::*;
pub use image_config::{ImageConfig, ImageHealthcheck};
pub use pack::pack_files_tar_zstd;
pub use unpack::*;

/// OCI manifest annotation key marking an image as a runtime-specific bundle.
///
/// Published by `zlayer vz build-base` (value [`ZLAYER_RUNTIME_VZ`]) and read by
/// the agent's composite runtime (`crates/zlayer-agent/src/runtimes/composite.rs`)
/// to auto-detect a macOS VZ base bundle and prefer the VZ runtime for it.
pub const ZLAYER_RUNTIME_ANNOTATION: &str = "com.zlayer.runtime";
/// [`ZLAYER_RUNTIME_ANNOTATION`] value for a macOS Apple-Virtualization bundle.
pub const ZLAYER_RUNTIME_VZ: &str = "vz";
/// [`ZLAYER_RUNTIME_ANNOTATION`] value for a macOS Apple-Virtualization
/// **Linux-guest** image (boots a Linux kernel under `Virtualization.framework`
/// via the agent's `macos_vz_linux` runtime). Distinct from [`ZLAYER_RUNTIME_VZ`],
/// which marks a native-macOS guest bundle.
pub const ZLAYER_RUNTIME_LINUX_VZ: &str = "vz-linux";
pub use wasm::*;

#[cfg(feature = "persistent")]
pub use persistent_cache::PersistentBlobCache;

#[cfg(feature = "s3")]
pub use s3_cache::{S3BlobCache, S3CacheConfig};

#[cfg(feature = "local")]
pub use local_registry::{ImageEntry, LocalRegistry, LocalRegistryError, RegistryIndex};

#[cfg(feature = "local")]
pub use oci_export::{
    export_image, import_image, import_image_from_bytes, ExportError, ExportInfo, ImportError,
    ImportInfo, OciDescriptor, OciIndex, OciLayout, OciManifest, OciPlatform,
};

pub use wasm_export::{export_wasm_as_oci, WasmExportConfig, WasmExportError, WasmExportResult};

// Re-export oci_client types that users need
pub use oci_client::manifest::OciImageManifest;
pub use oci_client::secrets::RegistryAuth;
