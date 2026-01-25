//! ZLayer Registry - OCI image pulling and caching
//!
//! This module provides OCI distribution client functionality with local blob caching.

pub mod cache;
pub mod client;
pub mod error;
pub mod unpack;

#[cfg(feature = "persistent")]
pub mod persistent_cache;

pub use cache::*;
pub use client::*;
pub use error::*;
pub use unpack::*;

#[cfg(feature = "persistent")]
pub use persistent_cache::PersistentBlobCache;

// Re-export oci_client types that users need
pub use oci_client::secrets::RegistryAuth;
