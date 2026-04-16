//! Example demonstrating persistent Raft storage
//!
//! Persistent storage is now provided by `zlayer-consensus` via its `zql-store` feature.
//! This example is kept as a placeholder to document the migration.
//!
//! To use persistent storage, enable the `zql-store` feature on `zlayer-consensus`
//! and use `zlayer_consensus_zql::storage::zql_store::ZqlLogStore` and
//! `zlayer_consensus_zql::storage::zql_store::ZqlStateMachine` instead of the
//! in-memory variants.

fn main() {
    println!("Persistent Raft storage has moved to the `zlayer-consensus` crate.");
    println!();
    println!("To use persistent storage:");
    println!("  1. Enable the `zql-store` feature on zlayer-consensus");
    println!("  2. Use ZqlLogStore + ZqlStateMachine from zlayer_consensus::storage::zql_store");
    println!("  3. Pass them to ConsensusNodeBuilder::build_with()");
}
