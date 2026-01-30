//! ZLayer Registry - OCI image pulling and caching
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

#[cfg(feature = "local")]
pub mod wasm_export;

pub use cache::*;
pub use cache_config::CacheType;
pub use client::*;
pub use error::*;
pub use unpack::*;
pub use wasm::*;

#[cfg(feature = "persistent")]
pub use persistent_cache::PersistentBlobCache;

#[cfg(feature = "s3")]
pub use s3_cache::{S3BlobCache, S3CacheConfig};

#[cfg(feature = "local")]
pub use local_registry::{ImageEntry, LocalRegistry, LocalRegistryError, RegistryIndex};

#[cfg(feature = "local")]
pub use oci_export::{
    export_image, import_image, ExportError, ExportInfo, ImportError, ImportInfo, OciDescriptor,
    OciIndex, OciLayout, OciManifest, OciPlatform,
};

#[cfg(feature = "local")]
pub use wasm_export::{export_wasm_as_oci, WasmExportConfig, WasmExportError, WasmExportResult};

// Re-export oci_client types that users need
pub use oci_client::secrets::RegistryAuth;
