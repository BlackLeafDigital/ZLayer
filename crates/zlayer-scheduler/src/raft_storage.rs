//! Storage re-exports from zlayer-consensus
//!
//! This module previously contained an inline openraft v1 `MemStore` implementation.
//! Storage is now provided by `zlayer_consensus::storage::mem_store`.
//!
//! This module is kept for backward-compatibility re-exports only.

// Re-export the consensus crate's mem store types under the old names for
// any code that still references them. New code should import directly from
// `zlayer_consensus::storage::mem_store`.

pub use zlayer_consensus::storage::mem_store::MemLogStore;
pub use zlayer_consensus::storage::mem_store::MemStateMachine;
pub use zlayer_consensus::storage::mem_store::SmData;
