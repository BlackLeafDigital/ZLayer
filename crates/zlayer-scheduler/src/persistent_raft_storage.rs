//! Persistent storage implementation for OpenRaft using ZQL
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
//! let storage = PersistentRaftStorage::new("/var/lib/zlayer/raft_zql").await?;
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
//! - **ZQL durability**: WAL-based storage with flush guarantees
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
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::raft::{ClusterState, NodeId, Response, TypeConfig};

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
// Error Helpers
// =============================================================================

/// Convert a generic error to OpenRaft StorageError
fn db_to_storage_error(
    subject: openraft::ErrorSubject<NodeId>,
    verb: openraft::ErrorVerb,
    msg: String,
) -> StorageError<NodeId> {
    StorageError::from_io_error(subject, verb, std::io::Error::other(msg))
}

/// Escape a string for safe ZQL query embedding
fn escape_str(s: &str) -> String {
    s.replace('\'', "''")
}

// =============================================================================
// Persistent Log Store
// =============================================================================

/// Persistent log storage backed by ZQL
pub struct PersistentLogStore {
    db: Arc<tokio::sync::Mutex<zql::Database>>,
}

impl Debug for PersistentLogStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersistentLogStore").finish()
    }
}

impl Clone for PersistentLogStore {
    fn clone(&self) -> Self {
        Self {
            db: Arc::clone(&self.db),
        }
    }
}

impl PersistentLogStore {
    /// Create or open a persistent log store
    pub fn new(db: Arc<tokio::sync::Mutex<zql::Database>>) -> Self {
        Self { db }
    }

    /// Get last purged log ID
    async fn get_last_purged(&self) -> Result<Option<LogId<NodeId>>, StorageError<NodeId>> {
        let mut db = self.db.lock().await;
        let result = db.query("SELECT * FROM log_metadata WHERE key = 'last_purged'");

        match result {
            Ok(zql::query::executor::ExecResult::Retrieved(records)) => {
                if records.is_empty() {
                    return Ok(None);
                }
                if let Some(data) = records[0].fields.get("value_data") {
                    let meta: LogMetadata = serde_json::from_str(data).map_err(|e| {
                        db_to_storage_error(
                            openraft::ErrorSubject::Store,
                            openraft::ErrorVerb::Read,
                            e.to_string(),
                        )
                    })?;
                    Ok(meta.last_purged_log_id)
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    /// Get last log entry
    async fn get_last_log(&self) -> Result<Option<Entry<TypeConfig>>, StorageError<NodeId>> {
        // We need to scan all log entries and find the one with the highest index
        let mut db = self.db.lock().await;
        let result = db.query("SELECT * FROM log_entries");

        match result {
            Ok(zql::query::executor::ExecResult::Retrieved(records)) => {
                if records.is_empty() {
                    return Ok(None);
                }
                // Find max index entry
                let mut best: Option<(u64, &zql::query::executor::RetrievedRecord)> = None;
                for record in &records {
                    if let Some(idx_str) = record.fields.get("log_index") {
                        if let Ok(idx) = idx_str.parse::<u64>() {
                            match best {
                                None => best = Some((idx, record)),
                                Some((cur_max, _)) if idx > cur_max => {
                                    best = Some((idx, record));
                                }
                                _ => {}
                            }
                        }
                    }
                }
                if let Some((_, record)) = best {
                    if let Some(data) = record.fields.get("entry_data") {
                        let entry: Entry<TypeConfig> = serde_json::from_str(data).map_err(|e| {
                            db_to_storage_error(
                                openraft::ErrorSubject::Store,
                                openraft::ErrorVerb::Read,
                                e.to_string(),
                            )
                        })?;
                        return Ok(Some(entry));
                    }
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }
}

// =============================================================================
// Persistent State Machine
// =============================================================================

/// Persistent state machine backed by ZQL
pub struct PersistentStateMachine {
    db: Arc<tokio::sync::Mutex<zql::Database>>,
}

impl Debug for PersistentStateMachine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersistentStateMachine").finish()
    }
}

impl Clone for PersistentStateMachine {
    fn clone(&self) -> Self {
        Self {
            db: Arc::clone(&self.db),
        }
    }
}

impl PersistentStateMachine {
    /// Create or open a persistent state machine
    pub fn new(db: Arc<tokio::sync::Mutex<zql::Database>>) -> Self {
        Self { db }
    }

    /// Get the current applied state
    async fn get_applied_state(&self) -> Result<AppliedState, StorageError<NodeId>> {
        let mut db = self.db.lock().await;
        let result = db.query("SELECT * FROM sm_applied_state WHERE key = 'current'");

        match result {
            Ok(zql::query::executor::ExecResult::Retrieved(records)) => {
                if records.is_empty() {
                    return Ok(AppliedState {
                        last_applied_log: None,
                        last_membership: StoredMembership::default(),
                        state: ClusterState::default(),
                    });
                }
                if let Some(data) = records[0].fields.get("value_data") {
                    let state: AppliedState = serde_json::from_str(data).map_err(|e| {
                        db_to_storage_error(
                            openraft::ErrorSubject::StateMachine,
                            openraft::ErrorVerb::Read,
                            e.to_string(),
                        )
                    })?;
                    Ok(state)
                } else {
                    Ok(AppliedState {
                        last_applied_log: None,
                        last_membership: StoredMembership::default(),
                        state: ClusterState::default(),
                    })
                }
            }
            _ => Ok(AppliedState {
                last_applied_log: None,
                last_membership: StoredMembership::default(),
                state: ClusterState::default(),
            }),
        }
    }

    /// Set the current applied state
    async fn set_applied_state(&self, state: &AppliedState) -> Result<(), StorageError<NodeId>> {
        let json = serde_json::to_string(state).map_err(|e| {
            db_to_storage_error(
                openraft::ErrorSubject::StateMachine,
                openraft::ErrorVerb::Write,
                e.to_string(),
            )
        })?;

        let mut db = self.db.lock().await;

        // Delete existing
        let _ = db.query("DELETE FROM sm_applied_state WHERE key = 'current'");

        // Insert new
        db.query(&format!(
            "INSERT INTO sm_applied_state (key, value_data) VALUES ('current', '{}')",
            escape_str(&json)
        ))
        .map_err(|e| {
            db_to_storage_error(
                openraft::ErrorSubject::StateMachine,
                openraft::ErrorVerb::Write,
                e.to_string(),
            )
        })?;

        // Flush for durability (ACID requirement for Raft)
        db.flush().map_err(|e| {
            db_to_storage_error(
                openraft::ErrorSubject::StateMachine,
                openraft::ErrorVerb::Write,
                e.to_string(),
            )
        })?;

        Ok(())
    }

    /// Get the current cluster state
    pub async fn get_state(&self) -> Result<ClusterState, StorageError<NodeId>> {
        let applied = self.get_applied_state().await?;
        Ok(applied.state)
    }
}

// =============================================================================
// Combined Persistent Storage
// =============================================================================

/// Combined persistent storage for OpenRaft (v1 API)
///
/// This implements the unified `RaftStorage` trait which combines
/// log storage and state machine operations, backed by ZQL.
pub struct PersistentRaftStorage {
    log_store: Arc<PersistentLogStore>,
    state_machine: Arc<RwLock<PersistentStateMachine>>,
    db: Arc<tokio::sync::Mutex<zql::Database>>,
    db_path: PathBuf,
}

impl PersistentRaftStorage {
    /// Create or open persistent storage at the given path
    pub async fn new(path: impl AsRef<Path>) -> Result<Self, StorageError<NodeId>> {
        let path = path.as_ref();

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Store,
                    openraft::ErrorVerb::Write,
                    e,
                )
            })?;
        }

        let db_path = path.to_path_buf();
        let db = tokio::task::spawn_blocking(move || zql::Database::open(&db_path))
            .await
            .map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Store,
                    openraft::ErrorVerb::Write,
                    std::io::Error::other(format!("spawn_blocking failed: {e}")),
                )
            })?
            .map_err(|e| {
                StorageError::from_io_error(
                    openraft::ErrorSubject::Store,
                    openraft::ErrorVerb::Write,
                    std::io::Error::other(format!(
                        "Failed to open database at {}: {e}",
                        path.display()
                    )),
                )
            })?;

        info!("Opened persistent Raft storage at {}", path.display());

        let db = Arc::new(tokio::sync::Mutex::new(db));

        let log_store = PersistentLogStore::new(Arc::clone(&db));
        let state_machine = PersistentStateMachine::new(Arc::clone(&db));

        Ok(Self {
            log_store: Arc::new(log_store),
            state_machine: Arc::new(RwLock::new(state_machine)),
            db,
            db_path: path.to_path_buf(),
        })
    }

    /// Get a reference to the state machine for reading cluster state
    pub fn state_machine(&self) -> Arc<RwLock<PersistentStateMachine>> {
        Arc::clone(&self.state_machine)
    }

    /// Get the underlying database path
    pub fn db_path(&self) -> PathBuf {
        self.db_path.clone()
    }
}

impl Clone for PersistentRaftStorage {
    fn clone(&self) -> Self {
        Self {
            log_store: Arc::clone(&self.log_store),
            state_machine: Arc::clone(&self.state_machine),
            db: Arc::clone(&self.db),
            db_path: self.db_path.clone(),
        }
    }
}

impl Debug for PersistentRaftStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersistentRaftStorage")
            .field("db_path", &self.db_path)
            .finish()
    }
}

// Implement RaftLogReader for reading log entries
impl RaftLogReader<TypeConfig> for PersistentRaftStorage {
    async fn try_get_log_entries<RB: RangeBounds<u64> + Clone + Debug + OptionalSend>(
        &mut self,
        range: RB,
    ) -> Result<Vec<Entry<TypeConfig>>, StorageError<NodeId>> {
        // Convert range bounds to concrete values
        let start = match range.start_bound() {
            std::ops::Bound::Included(&n) => n,
            std::ops::Bound::Excluded(&n) => n.saturating_add(1),
            std::ops::Bound::Unbounded => 0,
        };

        let end = match range.end_bound() {
            std::ops::Bound::Included(&n) => Some(n),
            std::ops::Bound::Excluded(&n) => n.checked_sub(1),
            std::ops::Bound::Unbounded => None,
        };

        // Fetch all log entries and filter in memory
        let mut db = self.db.lock().await;
        let result = db.query("SELECT * FROM log_entries");

        match result {
            Ok(zql::query::executor::ExecResult::Retrieved(records)) => {
                let mut entries: Vec<(u64, Entry<TypeConfig>)> = Vec::new();

                for record in &records {
                    if let (Some(idx_str), Some(data)) = (
                        record.fields.get("log_index"),
                        record.fields.get("entry_data"),
                    ) {
                        if let Ok(idx) = idx_str.parse::<u64>() {
                            let in_range = idx >= start
                                && match end {
                                    Some(e) => idx <= e,
                                    None => true,
                                };
                            if in_range {
                                let entry: Entry<TypeConfig> =
                                    serde_json::from_str(data).map_err(|e| {
                                        db_to_storage_error(
                                            openraft::ErrorSubject::Logs,
                                            openraft::ErrorVerb::Read,
                                            e.to_string(),
                                        )
                                    })?;
                                entries.push((idx, entry));
                            }
                        }
                    }
                }

                // Sort by index
                entries.sort_by_key(|(idx, _)| *idx);
                Ok(entries.into_iter().map(|(_, e)| e).collect())
            }
            _ => Ok(Vec::new()),
        }
    }
}

// Implement RaftSnapshotBuilder for creating snapshots
impl RaftSnapshotBuilder<TypeConfig> for PersistentRaftStorage {
    async fn build_snapshot(&mut self) -> Result<Snapshot<TypeConfig>, StorageError<NodeId>> {
        let sm = self.state_machine.read().await;
        let applied = sm.get_applied_state().await?;

        let data = serde_json::to_vec(&applied.state).map_err(|e| {
            db_to_storage_error(
                openraft::ErrorSubject::StateMachine,
                openraft::ErrorVerb::Read,
                e.to_string(),
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

        let meta_json = serde_json::to_string(&snapshot_record).map_err(|e| {
            db_to_storage_error(
                openraft::ErrorSubject::Snapshot(None),
                openraft::ErrorVerb::Write,
                e.to_string(),
            )
        })?;

        let data_hex = hex::encode(&data);

        // Save snapshot atomically
        let mut db = self.db.lock().await;

        // Delete old snapshot entries with same ID
        let _ = db.query(&format!(
            "DELETE FROM snapshot_metadata WHERE snapshot_id = '{}'",
            escape_str(&snapshot_id)
        ));
        let _ = db.query(&format!(
            "DELETE FROM snapshot_data WHERE snapshot_id = '{}'",
            escape_str(&snapshot_id)
        ));

        // Insert metadata
        db.query(&format!(
            "INSERT INTO snapshot_metadata (snapshot_id, metadata, created_at) VALUES ('{}', '{}', '{}')",
            escape_str(&snapshot_id),
            escape_str(&meta_json),
            now
        ))
        .map_err(|e| {
            db_to_storage_error(
                openraft::ErrorSubject::Snapshot(None),
                openraft::ErrorVerb::Write,
                e.to_string(),
            )
        })?;

        // Insert data
        db.query(&format!(
            "INSERT INTO snapshot_data (snapshot_id, data_hex) VALUES ('{}', '{}')",
            escape_str(&snapshot_id),
            escape_str(&data_hex)
        ))
        .map_err(|e| {
            db_to_storage_error(
                openraft::ErrorSubject::Snapshot(None),
                openraft::ErrorVerb::Write,
                e.to_string(),
            )
        })?;

        // Flush for durability
        db.flush().map_err(|e| {
            db_to_storage_error(
                openraft::ErrorSubject::Snapshot(None),
                openraft::ErrorVerb::Write,
                e.to_string(),
            )
        })?;

        debug!("Built snapshot {} ({} bytes)", snapshot_id, data.len());

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
        let json = serde_json::to_string(vote).map_err(|e| {
            db_to_storage_error(
                openraft::ErrorSubject::Vote,
                openraft::ErrorVerb::Write,
                e.to_string(),
            )
        })?;

        let mut db = self.db.lock().await;

        // Upsert
        let _ = db.query("DELETE FROM raft_vote WHERE key = 'current'");
        db.query(&format!(
            "INSERT INTO raft_vote (key, value_data) VALUES ('current', '{}')",
            escape_str(&json)
        ))
        .map_err(|e| {
            db_to_storage_error(
                openraft::ErrorSubject::Vote,
                openraft::ErrorVerb::Write,
                e.to_string(),
            )
        })?;

        // Flush for durability
        db.flush().map_err(|e| {
            db_to_storage_error(
                openraft::ErrorSubject::Vote,
                openraft::ErrorVerb::Write,
                e.to_string(),
            )
        })?;

        debug!("Saved vote: {:?}", vote);
        Ok(())
    }

    async fn read_vote(&mut self) -> Result<Option<Vote<NodeId>>, StorageError<NodeId>> {
        let mut db = self.db.lock().await;
        let result = db.query("SELECT * FROM raft_vote WHERE key = 'current'");

        match result {
            Ok(zql::query::executor::ExecResult::Retrieved(records)) => {
                if records.is_empty() {
                    return Ok(None);
                }
                if let Some(data) = records[0].fields.get("value_data") {
                    let vote: Vote<NodeId> = serde_json::from_str(data).map_err(|e| {
                        db_to_storage_error(
                            openraft::ErrorSubject::Vote,
                            openraft::ErrorVerb::Read,
                            e.to_string(),
                        )
                    })?;
                    Ok(Some(vote))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    // === Log operations ===

    async fn get_log_state(&mut self) -> Result<LogState<TypeConfig>, StorageError<NodeId>> {
        let last_purged = self.log_store.get_last_purged().await?;
        let last = self.log_store.get_last_log().await?.map(|e| e.log_id);

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
        let entries: Vec<_> = entries.into_iter().collect();
        if entries.is_empty() {
            return Ok(());
        }

        let mut db = self.db.lock().await;

        for entry in &entries {
            let json = serde_json::to_string(entry).map_err(|e| {
                db_to_storage_error(
                    openraft::ErrorSubject::Logs,
                    openraft::ErrorVerb::Write,
                    e.to_string(),
                )
            })?;

            // Upsert by index
            let _ = db.query(&format!(
                "DELETE FROM log_entries WHERE log_index = '{}'",
                entry.log_id.index
            ));

            db.query(&format!(
                "INSERT INTO log_entries (log_index, entry_data) VALUES ('{}', '{}')",
                entry.log_id.index,
                escape_str(&json)
            ))
            .map_err(|e| {
                db_to_storage_error(
                    openraft::ErrorSubject::Logs,
                    openraft::ErrorVerb::Write,
                    e.to_string(),
                )
            })?;
        }

        // Flush for durability
        db.flush().map_err(|e| {
            db_to_storage_error(
                openraft::ErrorSubject::Logs,
                openraft::ErrorVerb::Write,
                e.to_string(),
            )
        })?;

        debug!("Appended {} log entries", entries.len());
        Ok(())
    }

    async fn delete_conflict_logs_since(
        &mut self,
        log_id: LogId<NodeId>,
    ) -> Result<(), StorageError<NodeId>> {
        // We need to find and delete all entries with index >= log_id.index
        let mut db = self.db.lock().await;
        let result = db.query("SELECT * FROM log_entries");

        let indices_to_delete: Vec<u64> = match result {
            Ok(zql::query::executor::ExecResult::Retrieved(records)) => records
                .iter()
                .filter_map(|r| {
                    r.fields
                        .get("log_index")
                        .and_then(|s| s.parse::<u64>().ok())
                        .filter(|&idx| idx >= log_id.index)
                })
                .collect(),
            _ => Vec::new(),
        };

        for idx in &indices_to_delete {
            let _ = db.query(&format!(
                "DELETE FROM log_entries WHERE log_index = '{}'",
                idx
            ));
        }

        if !indices_to_delete.is_empty() {
            db.flush().map_err(|e| {
                db_to_storage_error(
                    openraft::ErrorSubject::Logs,
                    openraft::ErrorVerb::Write,
                    e.to_string(),
                )
            })?;
        }

        debug!(
            "Deleted {} conflict logs since index {}",
            indices_to_delete.len(),
            log_id.index
        );
        Ok(())
    }

    async fn purge_logs_upto(&mut self, log_id: LogId<NodeId>) -> Result<(), StorageError<NodeId>> {
        let mut db = self.db.lock().await;

        // Find and delete entries with index <= log_id.index
        let result = db.query("SELECT * FROM log_entries");

        let indices_to_delete: Vec<u64> = match result {
            Ok(zql::query::executor::ExecResult::Retrieved(records)) => records
                .iter()
                .filter_map(|r| {
                    r.fields
                        .get("log_index")
                        .and_then(|s| s.parse::<u64>().ok())
                        .filter(|&idx| idx <= log_id.index)
                })
                .collect(),
            _ => Vec::new(),
        };

        for idx in &indices_to_delete {
            let _ = db.query(&format!(
                "DELETE FROM log_entries WHERE log_index = '{}'",
                idx
            ));
        }

        // Update last purged log ID
        let meta = LogMetadata {
            last_purged_log_id: Some(log_id),
        };
        let meta_json = serde_json::to_string(&meta).map_err(|e| {
            db_to_storage_error(
                openraft::ErrorSubject::Logs,
                openraft::ErrorVerb::Write,
                e.to_string(),
            )
        })?;

        let _ = db.query("DELETE FROM log_metadata WHERE key = 'last_purged'");
        db.query(&format!(
            "INSERT INTO log_metadata (key, value_data) VALUES ('last_purged', '{}')",
            escape_str(&meta_json)
        ))
        .map_err(|e| {
            db_to_storage_error(
                openraft::ErrorSubject::Logs,
                openraft::ErrorVerb::Write,
                e.to_string(),
            )
        })?;

        // Flush for durability
        db.flush().map_err(|e| {
            db_to_storage_error(
                openraft::ErrorSubject::Logs,
                openraft::ErrorVerb::Write,
                e.to_string(),
            )
        })?;

        debug!(
            "Purged {} logs up to index {}",
            indices_to_delete.len(),
            log_id.index
        );
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
        let applied = sm.get_applied_state().await?;
        Ok((applied.last_applied_log, applied.last_membership))
    }

    async fn apply_to_state_machine(
        &mut self,
        entries: &[Entry<TypeConfig>],
    ) -> Result<Vec<Response>, StorageError<NodeId>> {
        let sm = self.state_machine.read().await;
        let mut applied = sm.get_applied_state().await?;
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
        sm.set_applied_state(&applied).await?;

        debug!("Applied {} entries to state machine", entries.len());
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
            db_to_storage_error(
                openraft::ErrorSubject::Snapshot(None),
                openraft::ErrorVerb::Read,
                e.to_string(),
            )
        })?;

        let applied = AppliedState {
            last_applied_log: meta.last_log_id,
            last_membership: meta.last_membership.clone(),
            state,
        };

        let sm = self.state_machine.write().await;
        sm.set_applied_state(&applied).await?;

        info!("Installed snapshot {}", meta.snapshot_id);
        Ok(())
    }

    async fn get_current_snapshot(
        &mut self,
    ) -> Result<Option<Snapshot<TypeConfig>>, StorageError<NodeId>> {
        // Get the latest snapshot by created_at timestamp
        let mut db = self.db.lock().await;
        let result = db.query("SELECT * FROM snapshot_metadata");

        let best_snapshot: Option<(String, String)> = match result {
            Ok(zql::query::executor::ExecResult::Retrieved(records)) => {
                let mut best: Option<(u64, String, String)> = None;
                for record in &records {
                    if let (Some(sid), Some(meta), Some(ts_str)) = (
                        record.fields.get("snapshot_id"),
                        record.fields.get("metadata"),
                        record.fields.get("created_at"),
                    ) {
                        let ts = ts_str.parse::<u64>().unwrap_or(0);
                        match best {
                            None => best = Some((ts, sid.clone(), meta.clone())),
                            Some((cur_ts, _, _)) if ts > cur_ts => {
                                best = Some((ts, sid.clone(), meta.clone()));
                            }
                            _ => {}
                        }
                    }
                }
                best.map(|(_, sid, meta)| (sid, meta))
            }
            _ => None,
        };

        if let Some((snapshot_id, meta_json)) = best_snapshot {
            let snapshot_record: SnapshotMetadataRecord = serde_json::from_str(&meta_json)
                .map_err(|e| {
                    db_to_storage_error(
                        openraft::ErrorSubject::Snapshot(None),
                        openraft::ErrorVerb::Read,
                        e.to_string(),
                    )
                })?;

            // Load snapshot data
            let data_result = db.query(&format!(
                "SELECT * FROM snapshot_data WHERE snapshot_id = '{}'",
                escape_str(&snapshot_id)
            ));

            if let Ok(zql::query::executor::ExecResult::Retrieved(records)) = data_result {
                if let Some(record) = records.first() {
                    if let Some(data_hex) = record.fields.get("data_hex") {
                        let data = hex::decode(data_hex).map_err(|e| {
                            db_to_storage_error(
                                openraft::ErrorSubject::Snapshot(None),
                                openraft::ErrorVerb::Read,
                                format!("hex decode failed: {e}"),
                            )
                        })?;

                        let meta = SnapshotMeta {
                            last_log_id: snapshot_record.last_log_id,
                            last_membership: snapshot_record.last_membership,
                            snapshot_id: snapshot_record.snapshot_id,
                        };

                        debug!(
                            "Loaded snapshot {} ({} bytes)",
                            meta.snapshot_id,
                            data.len()
                        );

                        return Ok(Some(Snapshot {
                            meta,
                            snapshot: Box::new(Cursor::new(data)),
                        }));
                    }
                }
            }
        }

        Ok(None)
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
        let db_path = temp_dir.path().join("test_raft_zql");
        let mut store = PersistentRaftStorage::new(&db_path).await.unwrap();

        // Check initial state
        let log_state = RaftStorage::get_log_state(&mut store).await.unwrap();
        assert!(log_state.last_log_id.is_none());
        assert!(log_state.last_purged_log_id.is_none());
    }

    #[tokio::test]
    async fn test_persistent_vote_operations() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_raft_zql");
        let mut store = PersistentRaftStorage::new(&db_path).await.unwrap();

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
        let db_path = temp_dir.path().join("test_raft_zql");
        let mut store = PersistentRaftStorage::new(&db_path).await.unwrap();

        let leader_id = CommittedLeaderId::new(1, 1);
        let log_id = LogId::new(leader_id, 1);

        let entry = Entry {
            log_id,
            payload: EntryPayload::Normal(Request::RegisterNode {
                node_id: 1,
                address: "127.0.0.1:8000".to_string(),
                wg_public_key: String::new(),
                overlay_ip: String::new(),
                overlay_port: 0,
                advertise_addr: String::new(),
                api_port: 0,
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
        let db_path = temp_dir.path().join("test_raft_zql");
        let mut store = PersistentRaftStorage::new(&db_path).await.unwrap();

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
        let state = sm.get_state().await.unwrap();
        let svc = state.get_service("test").unwrap();
        assert_eq!(svc.current_replicas, 3);
    }

    #[tokio::test]
    async fn test_persistent_snapshot() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_raft_zql");
        let mut store = PersistentRaftStorage::new(&db_path).await.unwrap();

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
        let db_path = temp_dir.path().join("test_raft_zql");

        // Create and write data
        {
            let mut store = PersistentRaftStorage::new(&db_path).await.unwrap();

            let new_vote = Vote::new(1, 1);
            RaftStorage::save_vote(&mut store, &new_vote).await.unwrap();

            let leader_id = CommittedLeaderId::new(1, 1);
            let log_id = LogId::new(leader_id, 1);

            let entry = Entry {
                log_id,
                payload: EntryPayload::Normal(Request::RegisterNode {
                    node_id: 1,
                    address: "127.0.0.1:8000".to_string(),
                    wg_public_key: String::new(),
                    overlay_ip: String::new(),
                    overlay_port: 0,
                    advertise_addr: String::new(),
                    api_port: 0,
                }),
            };

            RaftStorage::append_to_log(&mut store, vec![entry])
                .await
                .unwrap();
        }

        // Reopen and verify data persisted
        {
            let mut store = PersistentRaftStorage::new(&db_path).await.unwrap();

            let vote = RaftStorage::read_vote(&mut store).await.unwrap();
            assert_eq!(vote, Some(Vote::new(1, 1)));

            let mut reader = RaftStorage::get_log_reader(&mut store).await;
            let entries = reader.try_get_log_entries(0..2).await.unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].log_id.index, 1);
        }
    }

    #[tokio::test]
    async fn test_log_purge() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_raft_zql");
        let mut store = PersistentRaftStorage::new(&db_path).await.unwrap();

        let leader_id = CommittedLeaderId::new(1, 1);

        // Append multiple entries
        let entries: Vec<Entry<TypeConfig>> = (1..=5)
            .map(|i| Entry {
                log_id: LogId::new(leader_id, i),
                payload: EntryPayload::Normal(Request::RegisterNode {
                    node_id: i,
                    address: format!("127.0.0.1:{}", 8000 + i),
                    wg_public_key: String::new(),
                    overlay_ip: String::new(),
                    overlay_port: 0,
                    advertise_addr: String::new(),
                    api_port: 0,
                }),
            })
            .collect();

        RaftStorage::append_to_log(&mut store, entries)
            .await
            .unwrap();

        // Verify all entries exist
        let mut reader = RaftStorage::get_log_reader(&mut store).await;
        let all_entries = reader.try_get_log_entries(0..10).await.unwrap();
        assert_eq!(all_entries.len(), 5);

        // Purge up to index 3
        let purge_log_id = LogId::new(leader_id, 3);
        RaftStorage::purge_logs_upto(&mut store, purge_log_id)
            .await
            .unwrap();

        // Verify only entries 4 and 5 remain
        let mut reader = RaftStorage::get_log_reader(&mut store).await;
        let remaining = reader.try_get_log_entries(0..10).await.unwrap();
        assert_eq!(remaining.len(), 2);
        assert_eq!(remaining[0].log_id.index, 4);
        assert_eq!(remaining[1].log_id.index, 5);

        // Verify last_purged is updated
        let log_state = RaftStorage::get_log_state(&mut store).await.unwrap();
        assert_eq!(log_state.last_purged_log_id, Some(purge_log_id));
    }

    #[tokio::test]
    async fn test_delete_conflict_logs() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_raft_zql");
        let mut store = PersistentRaftStorage::new(&db_path).await.unwrap();

        let leader_id = CommittedLeaderId::new(1, 1);

        // Append multiple entries
        let entries: Vec<Entry<TypeConfig>> = (1..=5)
            .map(|i| Entry {
                log_id: LogId::new(leader_id, i),
                payload: EntryPayload::Blank,
            })
            .collect();

        RaftStorage::append_to_log(&mut store, entries)
            .await
            .unwrap();

        // Delete conflict logs since index 3
        let conflict_log_id = LogId::new(leader_id, 3);
        RaftStorage::delete_conflict_logs_since(&mut store, conflict_log_id)
            .await
            .unwrap();

        // Verify only entries 1 and 2 remain
        let mut reader = RaftStorage::get_log_reader(&mut store).await;
        let remaining = reader.try_get_log_entries(0..10).await.unwrap();
        assert_eq!(remaining.len(), 2);
        assert_eq!(remaining[0].log_id.index, 1);
        assert_eq!(remaining[1].log_id.index, 2);
    }

    #[tokio::test]
    async fn test_db_path_accessor() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_raft_zql");
        let store = PersistentRaftStorage::new(&db_path).await.unwrap();

        assert_eq!(store.db_path(), db_path);
    }
}
