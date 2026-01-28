//! Persistent storage implementation for OpenRaft using redb
//!
//! Provides durable log storage and state machine for the scheduler's Raft consensus.
//! Uses the RaftStorage v1 API which combines log and state machine storage.
//!
//! # Usage
//!
//! ```no_run
//! use zlayer_scheduler::persistent_raft_storage::PersistentRaftStorage;
//! use openraft::storage::RaftStorage;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Create or open persistent storage
//! let storage = PersistentRaftStorage::new("/var/lib/zlayer/raft.db")?;
//!
//! // Storage implements RaftStorage and can be used with OpenRaft
//! // Storage automatically recovers state on restart
//! # Ok(())
//! # }
//! ```
//!
//! # Features
//!
//! - **Crash recovery**: Survives process restarts without data loss
//! - **Snapshot support**: Efficient state snapshots for faster recovery
//! - **ACID transactions**: redb provides transactional guarantees
//! - **No external dependencies**: Embedded database, no separate server needed

// Allow large error types - OpenRaft's StorageError is inherently large
#![allow(clippy::result_large_err)]

use std::fmt::Debug;
use std::io::Cursor;
use std::ops::RangeBounds;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use openraft::storage::{LogState, RaftStorage, Snapshot};
use openraft::{
    Entry, EntryPayload, LogId, OptionalSend, RaftLogReader, RaftSnapshotBuilder, SnapshotMeta,
    StorageError, StoredMembership, Vote,
};
use redb::{Database, ReadableTable, TableDefinition};
use tokio::sync::RwLock;

use crate::raft::{ClusterState, NodeId, Response, TypeConfig};

// =============================================================================
// Table Definitions
// =============================================================================

/// Log entries: log index -> serialized Entry
const LOG_ENTRIES: TableDefinition<u64, &[u8]> = TableDefinition::new("log_entries");

/// Log metadata: stores last purged log ID
const LOG_METADATA: TableDefinition<&str, &[u8]> = TableDefinition::new("log_metadata");

/// Raft vote: stores current term and voted_for
const RAFT_VOTE: TableDefinition<&str, &[u8]> = TableDefinition::new("raft_vote");

/// Snapshot metadata: snapshot_id -> metadata
const SNAPSHOT_METADATA: TableDefinition<&str, &[u8]> = TableDefinition::new("snapshot_metadata");

/// Snapshot data: snapshot_id -> serialized ClusterState
const SNAPSHOT_DATA: TableDefinition<&str, &[u8]> = TableDefinition::new("snapshot_data");

/// State machine applied state: stores last applied log ID and membership
const SM_APPLIED_STATE: TableDefinition<&str, &[u8]> = TableDefinition::new("sm_applied_state");

// =============================================================================
// Serialization Helpers
// =============================================================================

/// Metadata for log purge operations
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct LogMetadata {
    last_purged_log_id: Option<LogId<NodeId>>,
}

/// Snapshot metadata for storage
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SnapshotMetadataRecord {
    last_log_id: Option<LogId<NodeId>>,
    last_membership: StoredMembership<NodeId, openraft::BasicNode>,
    snapshot_id: String,
    created_at: u64,
    size: u64,
}

/// State machine applied state
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct AppliedState {
    last_applied_log: Option<LogId<NodeId>>,
    last_membership: StoredMembership<NodeId, openraft::BasicNode>,
    state: ClusterState,
}

// =============================================================================
// Persistent Log Store
// =============================================================================

/// Persistent log storage backed by redb
#[derive(Debug)]
pub struct PersistentLogStore {
    db: Arc<Database>,
}

impl PersistentLogStore {
    /// Create or open a persistent log store
    pub fn new(path: impl AsRef<Path>) -> Result<Self, StorageError<NodeId>> {
        let db = Database::create(path.as_ref()).map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Store,
                openraft::ErrorVerb::Write,
                std::io::Error::other(e),
            )
        })?;

        // Initialize tables
        let write_txn = db.begin_write().map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Store,
                openraft::ErrorVerb::Write,
                std::io::Error::other(e),
            )
        })?;

        {
            write_txn.open_table(LOG_ENTRIES).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Store,
                    openraft::ErrorVerb::Write,
                    std::io::Error::other(e),
                )
            })?;

            write_txn.open_table(LOG_METADATA).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Store,
                    openraft::ErrorVerb::Write,
                    std::io::Error::other(e),
                )
            })?;

            write_txn.open_table(RAFT_VOTE).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Store,
                    openraft::ErrorVerb::Write,
                    std::io::Error::other(e),
                )
            })?;

            write_txn.open_table(SNAPSHOT_METADATA).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Store,
                    openraft::ErrorVerb::Write,
                    std::io::Error::other(e),
                )
            })?;

            write_txn.open_table(SNAPSHOT_DATA).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Store,
                    openraft::ErrorVerb::Write,
                    std::io::Error::other(e),
                )
            })?;

            write_txn.open_table(SM_APPLIED_STATE).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Store,
                    openraft::ErrorVerb::Write,
                    std::io::Error::other(e),
                )
            })?;
        }

        write_txn.commit().map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Store,
                openraft::ErrorVerb::Write,
                std::io::Error::other(e),
            )
        })?;

        Ok(Self { db: Arc::new(db) })
    }

    /// Get last purged log ID
    fn get_last_purged(&self) -> Result<Option<LogId<NodeId>>, StorageError<NodeId>> {
        let read_txn = self.db.begin_read().map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Store,
                openraft::ErrorVerb::Read,
                std::io::Error::other(e),
            )
        })?;

        let table = read_txn.open_table(LOG_METADATA).map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Store,
                openraft::ErrorVerb::Read,
                std::io::Error::other(e),
            )
        })?;

        let metadata = table.get("last_purged").map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Store,
                openraft::ErrorVerb::Read,
                std::io::Error::other(e),
            )
        })?;

        if let Some(bytes) = metadata {
            let meta: LogMetadata = serde_json::from_slice(bytes.value()).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Store,
                    openraft::ErrorVerb::Read,
                    std::io::Error::new(std::io::ErrorKind::InvalidData, e),
                )
            })?;
            Ok(meta.last_purged_log_id)
        } else {
            Ok(None)
        }
    }

    /// Set last purged log ID
    fn set_last_purged(&self, log_id: LogId<NodeId>) -> Result<(), StorageError<NodeId>> {
        let write_txn = self.db.begin_write().map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Store,
                openraft::ErrorVerb::Write,
                std::io::Error::other(e),
            )
        })?;

        {
            let mut table = write_txn.open_table(LOG_METADATA).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Store,
                    openraft::ErrorVerb::Write,
                    std::io::Error::other(e),
                )
            })?;

            let meta = LogMetadata {
                last_purged_log_id: Some(log_id),
            };

            let bytes = serde_json::to_vec(&meta).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Store,
                    openraft::ErrorVerb::Write,
                    std::io::Error::other(e),
                )
            })?;

            table.insert("last_purged", bytes.as_slice()).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Store,
                    openraft::ErrorVerb::Write,
                    std::io::Error::other(e),
                )
            })?;
        }

        write_txn.commit().map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Store,
                openraft::ErrorVerb::Write,
                std::io::Error::other(e),
            )
        })?;

        Ok(())
    }

    /// Get last log entry
    fn get_last_log(&self) -> Result<Option<Entry<TypeConfig>>, StorageError<NodeId>> {
        let read_txn = self.db.begin_read().map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Store,
                openraft::ErrorVerb::Read,
                std::io::Error::other(e),
            )
        })?;

        let table = read_txn.open_table(LOG_ENTRIES).map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Store,
                openraft::ErrorVerb::Read,
                std::io::Error::other(e),
            )
        })?;

        // Get the last entry using reverse iteration
        let last_entry = table
            .iter()
            .map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Store,
                    openraft::ErrorVerb::Read,
                    std::io::Error::other(e),
                )
            })?
            .last();

        if let Some(entry) = last_entry {
            let (_, value) = entry.map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Store,
                    openraft::ErrorVerb::Read,
                    std::io::Error::other(e),
                )
            })?;

            let entry: Entry<TypeConfig> = serde_json::from_slice(value.value()).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Store,
                    openraft::ErrorVerb::Read,
                    std::io::Error::new(std::io::ErrorKind::InvalidData, e),
                )
            })?;

            Ok(Some(entry))
        } else {
            Ok(None)
        }
    }
}

// =============================================================================
// Persistent State Machine
// =============================================================================

/// Persistent state machine backed by redb
#[derive(Debug)]
pub struct PersistentStateMachine {
    db: Arc<Database>,
}

impl PersistentStateMachine {
    /// Create or open a persistent state machine
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Get the current applied state
    fn get_applied_state(&self) -> Result<AppliedState, StorageError<NodeId>> {
        let read_txn = self.db.begin_read().map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::StateMachine,
                openraft::ErrorVerb::Read,
                std::io::Error::other(e),
            )
        })?;

        let table = read_txn.open_table(SM_APPLIED_STATE).map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::StateMachine,
                openraft::ErrorVerb::Read,
                std::io::Error::other(e),
            )
        })?;

        let state = table.get("current").map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::StateMachine,
                openraft::ErrorVerb::Read,
                std::io::Error::other(e),
            )
        })?;

        if let Some(bytes) = state {
            let state: AppliedState = serde_json::from_slice(bytes.value()).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::StateMachine,
                    openraft::ErrorVerb::Read,
                    std::io::Error::new(std::io::ErrorKind::InvalidData, e),
                )
            })?;
            Ok(state)
        } else {
            // Return default state if none exists
            Ok(AppliedState {
                last_applied_log: None,
                last_membership: StoredMembership::default(),
                state: ClusterState::default(),
            })
        }
    }

    /// Set the current applied state
    fn set_applied_state(&self, state: &AppliedState) -> Result<(), StorageError<NodeId>> {
        let write_txn = self.db.begin_write().map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::StateMachine,
                openraft::ErrorVerb::Write,
                std::io::Error::other(e),
            )
        })?;

        {
            let mut table = write_txn.open_table(SM_APPLIED_STATE).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::StateMachine,
                    openraft::ErrorVerb::Write,
                    std::io::Error::other(e),
                )
            })?;

            let bytes = serde_json::to_vec(state).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::StateMachine,
                    openraft::ErrorVerb::Write,
                    std::io::Error::other(e),
                )
            })?;

            table.insert("current", bytes.as_slice()).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::StateMachine,
                    openraft::ErrorVerb::Write,
                    std::io::Error::other(e),
                )
            })?;
        }

        write_txn.commit().map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::StateMachine,
                openraft::ErrorVerb::Write,
                std::io::Error::other(e),
            )
        })?;

        Ok(())
    }

    /// Get the current cluster state
    pub fn get_state(&self) -> Result<ClusterState, StorageError<NodeId>> {
        let applied = self.get_applied_state()?;
        Ok(applied.state)
    }
}

// =============================================================================
// Combined Persistent Storage
// =============================================================================

/// Combined persistent storage for OpenRaft (v1 API)
///
/// This implements the unified `RaftStorage` trait which combines
/// log storage and state machine operations, backed by redb.
pub struct PersistentRaftStorage {
    log_store: Arc<PersistentLogStore>,
    state_machine: Arc<RwLock<PersistentStateMachine>>,
    db: Arc<Database>,
}

impl PersistentRaftStorage {
    /// Create or open persistent storage at the given path
    pub fn new(path: impl AsRef<Path>) -> Result<Self, StorageError<NodeId>> {
        let log_store = PersistentLogStore::new(path.as_ref())?;
        let db = Arc::clone(&log_store.db);
        let state_machine = PersistentStateMachine::new(Arc::clone(&db));

        Ok(Self {
            log_store: Arc::new(log_store),
            state_machine: Arc::new(RwLock::new(state_machine)),
            db,
        })
    }

    /// Get a reference to the state machine for reading cluster state
    pub fn state_machine(&self) -> Arc<RwLock<PersistentStateMachine>> {
        Arc::clone(&self.state_machine)
    }

    /// Get the underlying database path
    pub fn db_path(&self) -> PathBuf {
        // redb doesn't expose the path directly, so we'll need to track it
        // For now, return a placeholder
        PathBuf::from(".")
    }
}

impl Clone for PersistentRaftStorage {
    fn clone(&self) -> Self {
        Self {
            log_store: Arc::clone(&self.log_store),
            state_machine: Arc::clone(&self.state_machine),
            db: Arc::clone(&self.db),
        }
    }
}

impl Debug for PersistentRaftStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersistentRaftStorage").finish()
    }
}

// Implement RaftLogReader for reading log entries
impl RaftLogReader<TypeConfig> for PersistentRaftStorage {
    async fn try_get_log_entries<RB: RangeBounds<u64> + Clone + Debug + OptionalSend>(
        &mut self,
        range: RB,
    ) -> Result<Vec<Entry<TypeConfig>>, StorageError<NodeId>> {
        let read_txn = self.db.begin_read().map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Logs,
                openraft::ErrorVerb::Read,
                std::io::Error::other(e),
            )
        })?;

        let table = read_txn.open_table(LOG_ENTRIES).map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Logs,
                openraft::ErrorVerb::Read,
                std::io::Error::other(e),
            )
        })?;

        let mut entries = Vec::new();

        // Iterate over the range
        let iter = table.range(range).map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Logs,
                openraft::ErrorVerb::Read,
                std::io::Error::other(e),
            )
        })?;

        for item in iter {
            let (_, value) = item.map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Logs,
                    openraft::ErrorVerb::Read,
                    std::io::Error::other(e),
                )
            })?;

            let entry: Entry<TypeConfig> = serde_json::from_slice(value.value()).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Logs,
                    openraft::ErrorVerb::Read,
                    std::io::Error::new(std::io::ErrorKind::InvalidData, e),
                )
            })?;

            entries.push(entry);
        }

        Ok(entries)
    }
}

// Implement RaftSnapshotBuilder for creating snapshots
impl RaftSnapshotBuilder<TypeConfig> for PersistentRaftStorage {
    async fn build_snapshot(&mut self) -> Result<Snapshot<TypeConfig>, StorageError<NodeId>> {
        let sm = self.state_machine.read().await;
        let applied = sm.get_applied_state()?;

        let data = serde_json::to_vec(&applied.state).map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::StateMachine,
                openraft::ErrorVerb::Read,
                std::io::Error::other(e),
            )
        })?;

        let snapshot_id = if let Some(last) = applied.last_applied_log {
            format!("{}-{}", last.leader_id, last.index)
        } else {
            "0-0".to_string()
        };

        // Store snapshot metadata
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let snapshot_record = SnapshotMetadataRecord {
            last_log_id: applied.last_applied_log,
            last_membership: applied.last_membership.clone(),
            snapshot_id: snapshot_id.clone(),
            created_at: now,
            size: data.len() as u64,
        };

        // Save snapshot to database
        let write_txn = self.db.begin_write().map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Snapshot(None),
                openraft::ErrorVerb::Write,
                std::io::Error::other(e),
            )
        })?;

        {
            let mut meta_table = write_txn.open_table(SNAPSHOT_METADATA).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Snapshot(None),
                    openraft::ErrorVerb::Write,
                    std::io::Error::other(e),
                )
            })?;

            let mut data_table = write_txn.open_table(SNAPSHOT_DATA).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Snapshot(None),
                    openraft::ErrorVerb::Write,
                    std::io::Error::other(e),
                )
            })?;

            let meta_bytes = serde_json::to_vec(&snapshot_record).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Snapshot(None),
                    openraft::ErrorVerb::Write,
                    std::io::Error::other(e),
                )
            })?;

            meta_table
                .insert(snapshot_id.as_str(), meta_bytes.as_slice())
                .map_err(|e| {
                    StorageError::from_io_error(
                        openraft::ErrorSubject::Snapshot(None),
                        openraft::ErrorVerb::Write,
                        std::io::Error::other(e),
                    )
                })?;

            data_table
                .insert(snapshot_id.as_str(), data.as_slice())
                .map_err(|e| {
                    StorageError::from_io_error(
                        openraft::ErrorSubject::Snapshot(None),
                        openraft::ErrorVerb::Write,
                        std::io::Error::other(e),
                    )
                })?;
        }

        write_txn.commit().map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Snapshot(None),
                openraft::ErrorVerb::Write,
                std::io::Error::other(e),
            )
        })?;

        let meta = SnapshotMeta {
            last_log_id: applied.last_applied_log,
            last_membership: applied.last_membership,
            snapshot_id,
        };

        Ok(Snapshot {
            meta,
            snapshot: Box::new(Cursor::new(data)),
        })
    }
}

// Implement the unified RaftStorage trait (v1 API)
#[allow(deprecated)]
impl RaftStorage<TypeConfig> for PersistentRaftStorage {
    type LogReader = Self;
    type SnapshotBuilder = Self;

    // === Vote operations ===

    async fn save_vote(&mut self, vote: &Vote<NodeId>) -> Result<(), StorageError<NodeId>> {
        let write_txn = self.db.begin_write().map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Vote,
                openraft::ErrorVerb::Write,
                std::io::Error::other(e),
            )
        })?;

        {
            let mut table = write_txn.open_table(RAFT_VOTE).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Vote,
                    openraft::ErrorVerb::Write,
                    std::io::Error::other(e),
                )
            })?;

            let bytes = serde_json::to_vec(vote).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Vote,
                    openraft::ErrorVerb::Write,
                    std::io::Error::other(e),
                )
            })?;

            table.insert("current", bytes.as_slice()).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Vote,
                    openraft::ErrorVerb::Write,
                    std::io::Error::other(e),
                )
            })?;
        }

        write_txn.commit().map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Vote,
                openraft::ErrorVerb::Write,
                std::io::Error::other(e),
            )
        })?;

        Ok(())
    }

    async fn read_vote(&mut self) -> Result<Option<Vote<NodeId>>, StorageError<NodeId>> {
        let read_txn = self.db.begin_read().map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Vote,
                openraft::ErrorVerb::Read,
                std::io::Error::other(e),
            )
        })?;

        let table = read_txn.open_table(RAFT_VOTE).map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Vote,
                openraft::ErrorVerb::Read,
                std::io::Error::other(e),
            )
        })?;

        let vote = table.get("current").map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Vote,
                openraft::ErrorVerb::Read,
                std::io::Error::other(e),
            )
        })?;

        if let Some(bytes) = vote {
            let vote: Vote<NodeId> = serde_json::from_slice(bytes.value()).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Vote,
                    openraft::ErrorVerb::Read,
                    std::io::Error::new(std::io::ErrorKind::InvalidData, e),
                )
            })?;
            Ok(Some(vote))
        } else {
            Ok(None)
        }
    }

    // === Log operations ===

    async fn get_log_state(&mut self) -> Result<LogState<TypeConfig>, StorageError<NodeId>> {
        let last_purged = self.log_store.get_last_purged()?;
        let last = self.log_store.get_last_log()?.map(|e| e.log_id);

        Ok(LogState {
            last_purged_log_id: last_purged,
            last_log_id: last,
        })
    }

    async fn get_log_reader(&mut self) -> Self::LogReader {
        self.clone()
    }

    async fn append_to_log<I>(&mut self, entries: I) -> Result<(), StorageError<NodeId>>
    where
        I: IntoIterator<Item = Entry<TypeConfig>> + OptionalSend,
    {
        let write_txn = self.db.begin_write().map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Logs,
                openraft::ErrorVerb::Write,
                std::io::Error::other(e),
            )
        })?;

        {
            let mut table = write_txn.open_table(LOG_ENTRIES).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Logs,
                    openraft::ErrorVerb::Write,
                    std::io::Error::other(e),
                )
            })?;

            for entry in entries {
                let bytes = serde_json::to_vec(&entry).map_err(|e| {
                    StorageError::from_io_error(
                        openraft::ErrorSubject::Logs,
                        openraft::ErrorVerb::Write,
                        std::io::Error::other(e),
                    )
                })?;

                table
                    .insert(entry.log_id.index, bytes.as_slice())
                    .map_err(|e| {
                        StorageError::from_io_error(
                            openraft::ErrorSubject::Logs,
                            openraft::ErrorVerb::Write,
                            std::io::Error::other(e),
                        )
                    })?;
            }
        }

        write_txn.commit().map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Logs,
                openraft::ErrorVerb::Write,
                std::io::Error::other(e),
            )
        })?;

        Ok(())
    }

    async fn delete_conflict_logs_since(
        &mut self,
        log_id: LogId<NodeId>,
    ) -> Result<(), StorageError<NodeId>> {
        let write_txn = self.db.begin_write().map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Logs,
                openraft::ErrorVerb::Write,
                std::io::Error::other(e),
            )
        })?;

        {
            let mut table = write_txn.open_table(LOG_ENTRIES).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Logs,
                    openraft::ErrorVerb::Write,
                    std::io::Error::other(e),
                )
            })?;

            // Collect keys to delete
            let keys_to_delete: Vec<u64> = table
                .range(log_id.index..)
                .map_err(|e| {
                    StorageError::from_io_error(
                        openraft::ErrorSubject::Logs,
                        openraft::ErrorVerb::Read,
                        std::io::Error::other(e),
                    )
                })?
                .map(|item| {
                    item.map(|(k, _)| k.value()).map_err(|e| {
                        StorageError::from_io_error(
                            openraft::ErrorSubject::Logs,
                            openraft::ErrorVerb::Read,
                            std::io::Error::other(e),
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;

            // Delete entries
            for key in keys_to_delete {
                table.remove(key).map_err(|e| {
                    StorageError::from_io_error(
                        openraft::ErrorSubject::Logs,
                        openraft::ErrorVerb::Write,
                        std::io::Error::other(e),
                    )
                })?;
            }
        }

        write_txn.commit().map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Logs,
                openraft::ErrorVerb::Write,
                std::io::Error::other(e),
            )
        })?;

        Ok(())
    }

    async fn purge_logs_upto(&mut self, log_id: LogId<NodeId>) -> Result<(), StorageError<NodeId>> {
        let write_txn = self.db.begin_write().map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Logs,
                openraft::ErrorVerb::Write,
                std::io::Error::other(e),
            )
        })?;

        {
            let mut table = write_txn.open_table(LOG_ENTRIES).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Logs,
                    openraft::ErrorVerb::Write,
                    std::io::Error::other(e),
                )
            })?;

            // Collect keys to delete
            let keys_to_delete: Vec<u64> = table
                .range(..=log_id.index)
                .map_err(|e| {
                    StorageError::from_io_error(
                        openraft::ErrorSubject::Logs,
                        openraft::ErrorVerb::Read,
                        std::io::Error::other(e),
                    )
                })?
                .map(|item| {
                    item.map(|(k, _)| k.value()).map_err(|e| {
                        StorageError::from_io_error(
                            openraft::ErrorSubject::Logs,
                            openraft::ErrorVerb::Read,
                            std::io::Error::other(e),
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;

            // Delete entries
            for key in keys_to_delete {
                table.remove(key).map_err(|e| {
                    StorageError::from_io_error(
                        openraft::ErrorSubject::Logs,
                        openraft::ErrorVerb::Write,
                        std::io::Error::other(e),
                    )
                })?;
            }
        }

        write_txn.commit().map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Logs,
                openraft::ErrorVerb::Write,
                std::io::Error::other(e),
            )
        })?;

        // Update last purged log ID
        self.log_store.set_last_purged(log_id)?;

        Ok(())
    }

    // === State machine operations ===

    async fn last_applied_state(
        &mut self,
    ) -> Result<
        (
            Option<LogId<NodeId>>,
            StoredMembership<NodeId, openraft::BasicNode>,
        ),
        StorageError<NodeId>,
    > {
        let sm = self.state_machine.read().await;
        let applied = sm.get_applied_state()?;
        Ok((applied.last_applied_log, applied.last_membership))
    }

    async fn apply_to_state_machine(
        &mut self,
        entries: &[Entry<TypeConfig>],
    ) -> Result<Vec<Response>, StorageError<NodeId>> {
        let sm = self.state_machine.read().await;
        let mut applied = sm.get_applied_state()?;
        let mut responses = Vec::new();

        for entry in entries {
            applied.last_applied_log = Some(entry.log_id);

            match &entry.payload {
                EntryPayload::Blank => {
                    responses.push(Response::Success { data: None });
                }
                EntryPayload::Normal(req) => {
                    let resp = applied.state.apply(req);
                    responses.push(resp);
                }
                EntryPayload::Membership(mem) => {
                    applied.last_membership =
                        StoredMembership::new(Some(entry.log_id), mem.clone());
                    responses.push(Response::Success { data: None });
                }
            }
        }

        // Persist the updated state
        sm.set_applied_state(&applied)?;

        Ok(responses)
    }

    // === Snapshot operations ===

    async fn get_snapshot_builder(&mut self) -> Self::SnapshotBuilder {
        self.clone()
    }

    async fn begin_receiving_snapshot(
        &mut self,
    ) -> Result<Box<Cursor<Vec<u8>>>, StorageError<NodeId>> {
        Ok(Box::new(Cursor::new(Vec::new())))
    }

    async fn install_snapshot(
        &mut self,
        meta: &SnapshotMeta<NodeId, openraft::BasicNode>,
        snapshot: Box<Cursor<Vec<u8>>>,
    ) -> Result<(), StorageError<NodeId>> {
        let data = snapshot.into_inner();
        let state: ClusterState = serde_json::from_slice(&data).map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Snapshot(None),
                openraft::ErrorVerb::Read,
                std::io::Error::new(std::io::ErrorKind::InvalidData, e),
            )
        })?;

        let applied = AppliedState {
            last_applied_log: meta.last_log_id,
            last_membership: meta.last_membership.clone(),
            state,
        };

        let sm = self.state_machine.write().await;
        sm.set_applied_state(&applied)?;

        Ok(())
    }

    async fn get_current_snapshot(
        &mut self,
    ) -> Result<Option<Snapshot<TypeConfig>>, StorageError<NodeId>> {
        // Try to load the latest snapshot from disk
        let read_txn = self.db.begin_read().map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Snapshot(None),
                openraft::ErrorVerb::Read,
                std::io::Error::other(e),
            )
        })?;

        let meta_table = read_txn.open_table(SNAPSHOT_METADATA).map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Snapshot(None),
                openraft::ErrorVerb::Read,
                std::io::Error::other(e),
            )
        })?;

        let data_table = read_txn.open_table(SNAPSHOT_DATA).map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Snapshot(None),
                openraft::ErrorVerb::Read,
                std::io::Error::other(e),
            )
        })?;

        // Get the latest snapshot (last entry)
        let latest_meta = meta_table
            .iter()
            .map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Snapshot(None),
                    openraft::ErrorVerb::Read,
                    std::io::Error::other(e),
                )
            })?
            .last();

        if let Some(meta_entry) = latest_meta {
            let (snapshot_id, meta_bytes) = meta_entry.map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Snapshot(None),
                    openraft::ErrorVerb::Read,
                    std::io::Error::other(e),
                )
            })?;

            let snapshot_record: SnapshotMetadataRecord =
                serde_json::from_slice(meta_bytes.value()).map_err(|e| {
                    StorageError::from_io_error(
                        openraft::ErrorSubject::Snapshot(None),
                        openraft::ErrorVerb::Read,
                        std::io::Error::new(std::io::ErrorKind::InvalidData, e),
                    )
                })?;

            // Load snapshot data
            let data_bytes = data_table.get(snapshot_id.value()).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Snapshot(None),
                    openraft::ErrorVerb::Read,
                    std::io::Error::other(e),
                )
            })?;

            if let Some(data) = data_bytes {
                let meta = SnapshotMeta {
                    last_log_id: snapshot_record.last_log_id,
                    last_membership: snapshot_record.last_membership,
                    snapshot_id: snapshot_record.snapshot_id,
                };

                Ok(Some(Snapshot {
                    meta,
                    snapshot: Box::new(Cursor::new(data.value().to_vec())),
                }))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raft::{Request, ServiceState};
    use openraft::CommittedLeaderId;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_persistent_storage_log_operations() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let mut store = PersistentRaftStorage::new(&db_path).unwrap();

        // Check initial state
        let log_state = RaftStorage::get_log_state(&mut store).await.unwrap();
        assert!(log_state.last_log_id.is_none());
        assert!(log_state.last_purged_log_id.is_none());
    }

    #[tokio::test]
    async fn test_persistent_vote_operations() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let mut store = PersistentRaftStorage::new(&db_path).unwrap();

        // Initially no vote
        let vote = RaftStorage::read_vote(&mut store).await.unwrap();
        assert!(vote.is_none());

        // Save a vote
        let new_vote = Vote::new(1, 1);
        RaftStorage::save_vote(&mut store, &new_vote).await.unwrap();

        // Read it back
        let vote = RaftStorage::read_vote(&mut store).await.unwrap();
        assert_eq!(vote, Some(new_vote));
    }

    #[tokio::test]
    async fn test_persistent_log_append_and_read() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let mut store = PersistentRaftStorage::new(&db_path).unwrap();

        let leader_id = CommittedLeaderId::new(1, 1);
        let log_id = LogId::new(leader_id, 1);

        let entry = Entry {
            log_id,
            payload: EntryPayload::Normal(Request::RegisterNode {
                node_id: 1,
                address: "127.0.0.1:8000".to_string(),
            }),
        };

        // Append it
        RaftStorage::append_to_log(&mut store, vec![entry.clone()])
            .await
            .unwrap();

        // Read it back
        let mut reader = RaftStorage::get_log_reader(&mut store).await;
        let entries = reader.try_get_log_entries(0..2).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].log_id.index, 1);
    }

    #[tokio::test]
    async fn test_persistent_state_machine_apply() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let mut store = PersistentRaftStorage::new(&db_path).unwrap();

        let leader_id = CommittedLeaderId::new(1, 1);
        let log_id = LogId::new(leader_id, 1);

        let entry = Entry {
            log_id,
            payload: EntryPayload::Normal(Request::UpdateServiceState {
                service_name: "test".to_string(),
                state: ServiceState {
                    current_replicas: 3,
                    ..Default::default()
                },
            }),
        };

        // Apply entry
        let responses = RaftStorage::apply_to_state_machine(&mut store, &[entry])
            .await
            .unwrap();

        assert_eq!(responses.len(), 1);

        // Verify state was persisted
        let sm = store.state_machine();
        let sm = sm.read().await;
        let state = sm.get_state().unwrap();
        let svc = state.get_service("test").unwrap();
        assert_eq!(svc.current_replicas, 3);
    }

    #[tokio::test]
    async fn test_persistent_snapshot() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let mut store = PersistentRaftStorage::new(&db_path).unwrap();

        // Build a snapshot
        let snapshot = RaftSnapshotBuilder::build_snapshot(&mut store)
            .await
            .unwrap();

        assert!(snapshot.meta.last_log_id.is_none());
        assert_eq!(snapshot.meta.snapshot_id, "0-0");

        // Verify snapshot was saved
        let current = RaftStorage::get_current_snapshot(&mut store).await.unwrap();
        assert!(current.is_some());
        let current = current.unwrap();
        assert_eq!(current.meta.snapshot_id, "0-0");
    }

    #[tokio::test]
    async fn test_persistent_storage_survives_restart() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        // Create and write data
        {
            let mut store = PersistentRaftStorage::new(&db_path).unwrap();

            let new_vote = Vote::new(1, 1);
            RaftStorage::save_vote(&mut store, &new_vote).await.unwrap();

            let leader_id = CommittedLeaderId::new(1, 1);
            let log_id = LogId::new(leader_id, 1);

            let entry = Entry {
                log_id,
                payload: EntryPayload::Normal(Request::RegisterNode {
                    node_id: 1,
                    address: "127.0.0.1:8000".to_string(),
                }),
            };

            RaftStorage::append_to_log(&mut store, vec![entry])
                .await
                .unwrap();
        }

        // Reopen and verify data persisted
        {
            let mut store = PersistentRaftStorage::new(&db_path).unwrap();

            let vote = RaftStorage::read_vote(&mut store).await.unwrap();
            assert_eq!(vote, Some(Vote::new(1, 1)));

            let mut reader = RaftStorage::get_log_reader(&mut store).await;
            let entries = reader.try_get_log_entries(0..2).await.unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].log_id.index, 1);
        }
    }
}
