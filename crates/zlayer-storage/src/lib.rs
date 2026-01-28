//! S3-backed container layer storage
//!
//! Provides persistent storage for container filesystem changes (OverlayFS upper layer)
//! with crash-tolerant uploads and resume capability.

pub mod config;
pub mod error;
pub mod snapshot;
pub mod sync;
pub mod types;

pub use config::*;
pub use error::*;
pub use types::*;
