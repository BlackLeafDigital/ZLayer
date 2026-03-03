//! Example demonstrating persistent Raft storage
//!
//! Persistent storage is now provided by `zlayer-consensus` via its `redb-store` feature.
//! This example is kept as a placeholder to document the migration.
//!
//! To use persistent storage, enable the `redb-store` feature on `zlayer-consensus`
//! and use `zlayer_consensus::storage::redb_store::RedbLogStore` and
//! `zlayer_consensus::storage::redb_store::RedbStateMachine` instead of the
//! in-memory variants.

fn main() {
    println!("Persistent Raft storage has moved to the `zlayer-consensus` crate.");
    println!();
    println!("To use persistent storage:");
    println!("  1. Enable the `redb-store` feature on zlayer-consensus");
    println!("  2. Use RedbLogStore + RedbStateMachine from zlayer_consensus::storage::redb_store");
    println!("  3. Pass them to ConsensusNodeBuilder::build_with()");
}
