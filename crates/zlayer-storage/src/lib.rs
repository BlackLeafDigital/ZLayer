//! S3-backed container layer storage
//!
//! Provides persistent storage for container filesystem changes (OverlayFS upper layer)
//! with crash-tolerant uploads and resume capability.
//!
//! # Features
//!
//! - **Layer Sync**: Synchronize OverlayFS upper layers to S3 with crash recovery
//! - **SQLite Replication**: WAL-based SQLite database replication to S3
//!
//! # Modules
//!
//! - [`sync`]: Container layer synchronization with S3
//! - [`replicator`]: SQLite WAL-based replication to S3
//! - [`snapshot`]: Snapshot creation and extraction utilities

pub mod config;
pub mod error;
pub mod replicator;
pub mod snapshot;
pub mod sync;
pub mod types;

pub use config::*;
pub use error::*;
pub use replicator::{
    CacheEntry, ReplicationMetadata, ReplicationStatus, SqliteReplicator, SqliteReplicatorConfig,
    WalEvent,
};
pub use types::*;
