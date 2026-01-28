//! Example demonstrating persistent Raft storage
//!
//! This example shows how to use PersistentRaftStorage instead of MemStore
//! for durable consensus state.
//!
//! Run with: cargo run --example persistent_storage --features persistent

#[cfg(feature = "persistent")]
use zlayer_scheduler::persistent_raft_storage::PersistentRaftStorage;

#[tokio::main]
#[cfg(feature = "persistent")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use openraft::storage::Adaptor;

    // Create persistent storage in /tmp for this example
    let storage = PersistentRaftStorage::new("/tmp/zlayer-raft.db")?;

    // Create adaptors for OpenRaft
    let (_log_store, _state_machine) = Adaptor::new(storage);

    println!("Persistent Raft storage created at /tmp/zlayer-raft.db");
    println!("Storage will survive process restarts!");

    // The log_store and state_machine can now be used with OpenRaft
    // In a real application, you would pass these to Raft::new() instead of MemStore

    println!("Example Raft configuration:");
    println!("\nNote: To use persistent storage in production:");
    println!("1. Replace MemStore::new() with PersistentRaftStorage::new(path)");
    println!("2. Create adaptors with Adaptor::new(storage)");
    println!("3. Pass adaptors to Raft::new() instead of the MemStore adaptors");

    Ok(())
}

#[cfg(not(feature = "persistent"))]
fn main() {
    eprintln!("This example requires the 'persistent' feature.");
    eprintln!("Run with: cargo run --example persistent_storage --features persistent");
    std::process::exit(1);
}
