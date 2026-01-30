//! HTTP handlers for Raft RPCs

pub mod raft;

pub use raft::{handle_append_entries, handle_full_snapshot, handle_install_snapshot, handle_vote};
