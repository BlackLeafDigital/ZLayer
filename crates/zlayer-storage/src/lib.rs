//! S3-backed container layer storage
//!
//! Provides persistent storage for container filesystem changes (`OverlayFS` upper layer)
//! with crash-tolerant uploads and resume capability.
//!
//! # Features
//!
//! - **Layer Sync**: Synchronize `OverlayFS` upper layers to S3 with crash recovery
//!
//! # Modules
//!
//! - [`sync`]: Container layer synchronization with S3
//! - [`snapshot`]: Snapshot creation and extraction utilities

pub mod config;
pub mod error;
pub mod snapshot;
pub mod sync;
pub mod types;

pub use config::*;
pub use error::*;
pub use types::*;
