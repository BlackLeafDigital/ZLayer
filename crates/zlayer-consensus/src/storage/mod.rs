//! Storage implementations for the Raft log and state machine.
//!
//! Two backends are provided:
//!
//! - **`MemStore`** (feature `mem-store`, default): BTreeMap-based in-memory storage.
//!   Suitable for testing and development. Data is lost on restart.
//!
//! - **`RedbStore`** (feature `redb-store`): Crash-safe persistent storage backed by
//!   the `redb` embedded database. Suitable for production deployments.

#[cfg(feature = "mem-store")]
pub mod mem_store;

#[cfg(feature = "redb-store")]
pub mod redb_store;
