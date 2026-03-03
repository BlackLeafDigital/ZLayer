//! Storage implementations for the Raft log and state machine.
//!
//! Two backends are provided:
//!
//! - **`MemStore`** (feature `mem-store`, default): BTreeMap-based in-memory storage.
//!   Suitable for testing and development. Data is lost on restart.
//!
//! - **`ZqlStore`** (feature `zql-store`): Crash-safe persistent storage backed by
//!   the ZQL embedded database. Suitable for production deployments.

#[cfg(feature = "mem-store")]
pub mod mem_store;

#[cfg(feature = "zql-store")]
pub mod zql_store;
