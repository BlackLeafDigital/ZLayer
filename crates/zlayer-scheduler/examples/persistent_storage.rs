//! Example demonstrating persistent Raft storage
//!
//! This example shows how to use PersistentRaftStorage instead of MemStore
//! for durable consensus state.
//!
//! Run with: cargo run --example persistent_storage --features persistent

#[cfg(feature = "persistent")]
use zlayer_scheduler_zql::persistent_raft_storage::PersistentRaftStorage;

#[tokio::main]
#[cfg(feature = "persistent")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create persistent storage in /tmp for this example
    let _storage = PersistentRaftStorage::new("/tmp/zlayer-raft-zql").await?;

    println!("Persistent Raft storage created at /tmp/zlayer-raft-zql");
    println!("Storage will survive process restarts!");

    // PersistentRaftStorage directly implements both RaftLogStorage and RaftStateMachine.
    // In a real application, you would pass it (or an adaptor over it) to Raft::new().

    println!("Example Raft configuration:");
    println!("\nNote: To use persistent storage in production:");
    println!("1. Replace MemStore::new() with PersistentRaftStorage::new(path)");
    println!("2. PersistentRaftStorage implements RaftLogStorage + RaftStateMachine directly");
    println!("3. Pass the storage to Raft::new() instead of the MemStore");

    Ok(())
}

#[cfg(not(feature = "persistent"))]
fn main() {
    eprintln!("This example requires the 'persistent' feature.");
    eprintln!("Run with: cargo run --example persistent_storage --features persistent");
    std::process::exit(1);
}
