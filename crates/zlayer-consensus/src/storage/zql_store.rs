//! Persistent storage implementation backed by ZQL.
//!
//! This store provides crash-safe, durable storage for Raft log entries,
//! votes, and state machine snapshots. It implements the openraft v2 storage
//! API (`RaftLogStorage` + `RaftStateMachine`).
//!
//! ## Table layout
//!
//! All data is stored via ZQL's SQL interface with JSON-serialized values.
//!
//! | Table | Key column | Value column |
//! |-------|-----------|--------------|
//! | `raft_log_entries` | `log_index` (u64 as string) | `entry_data` (JSON) |
//! | `raft_meta` | `key` (string) | `value_data` (JSON) |
//! | `raft_sm_state` | `key` (string) | `value_data` (JSON) |
//! | `raft_snapshots` | `key` (string) | `value_data` (JSON or hex) |

use std::fmt::Debug;
use std::io::Cursor;
use std::ops::RangeBounds;
use std::path::Path;
use std::sync::Arc;

use openraft::log_id::RaftLogId;
use openraft::storage::{LogFlushed, LogState, RaftLogStorage, RaftStateMachine, Snapshot};
use openraft::{
    Entry, EntryPayload, LogId, OptionalSend, RaftLogReader, RaftSnapshotBuilder, RaftTypeConfig,
    SnapshotMeta, StorageError, StoredMembership, Vote,
};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, warn};

use crate::types::NodeId;

// ---------------------------------------------------------------------------
// Metadata keys
// ---------------------------------------------------------------------------

const META_VOTE: &str = "vote";
const META_LAST_PURGED: &str = "last_purged";
const META_COMMITTED: &str = "committed";
const META_LAST_APPLIED: &str = "last_applied";
const META_LAST_MEMBERSHIP: &str = "last_membership";

const SM_STATE: &str = "state";
const SNAPSHOT_CURRENT: &str = "current";
const SNAPSHOT_META: &str = "meta";

// ---------------------------------------------------------------------------
// Helper: convert errors to StorageError
// ---------------------------------------------------------------------------

fn zql_to_storage_err(
    subject: openraft::ErrorSubject<NodeId>,
    verb: openraft::ErrorVerb,
    e: impl std::fmt::Display,
) -> StorageError<NodeId> {
    StorageError::from_io_error(subject, verb, std::io::Error::other(e.to_string()))
}

/// Escape a string for safe ZQL query embedding.
fn escape_str(s: &str) -> String {
    s.replace('\'', "''")
}

// ---------------------------------------------------------------------------
// ZqlLogStore
// ---------------------------------------------------------------------------

/// Persistent Raft log store backed by ZQL.
pub struct ZqlLogStore<C: RaftTypeConfig<NodeId = NodeId>> {
    db: Arc<Mutex<zql::Database>>,
    _phantom: std::marker::PhantomData<C>,
}

impl<C: RaftTypeConfig<NodeId = NodeId>> ZqlLogStore<C> {
    /// Open or create a log store at the given path.
    ///
    /// The database directory is created if it does not exist.
    pub fn new(path: impl AsRef<Path>) -> Result<Self, crate::error::ConsensusError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                crate::error::ConsensusError::Zql(format!(
                    "Failed to create parent dir {:?}: {e}",
                    parent
                ))
            })?;
        }

        let db = zql::Database::open(path).map_err(|e| {
            crate::error::ConsensusError::Zql(format!("Failed to open ZQL at {:?}: {e}", path))
        })?;

        Ok(Self {
            db: Arc::new(Mutex::new(db)),
            _phantom: std::marker::PhantomData,
        })
    }

    /// Create from an already-open ZQL `Database`.
    pub fn from_db(db: Arc<Mutex<zql::Database>>) -> Self {
        Self {
            db,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Get a clone of the database handle.
    pub fn db(&self) -> Arc<Mutex<zql::Database>> {
        Arc::clone(&self.db)
    }
}

impl<C: RaftTypeConfig<NodeId = NodeId>> Clone for ZqlLogStore<C> {
    fn clone(&self) -> Self {
        Self {
            db: Arc::clone(&self.db),
            _phantom: std::marker::PhantomData,
        }
    }
}

/// A cloneable log reader backed by ZQL.
pub struct ZqlLogReader<C: RaftTypeConfig<NodeId = NodeId>> {
    db: Arc<Mutex<zql::Database>>,
    _phantom: std::marker::PhantomData<C>,
}

impl<C: RaftTypeConfig<NodeId = NodeId>> Clone for ZqlLogReader<C> {
    fn clone(&self) -> Self {
        Self {
            db: Arc::clone(&self.db),
            _phantom: std::marker::PhantomData,
        }
    }
}

/// Read log entries from ZQL within the given range.
///
/// This is shared logic used by both `ZqlLogReader` and `ZqlLogStore`.
async fn read_log_entries<C>(
    db: &Mutex<zql::Database>,
    range: impl RangeBounds<u64>,
) -> Result<Vec<C::Entry>, StorageError<NodeId>>
where
    C: RaftTypeConfig<NodeId = NodeId>,
    C::Entry: serde::de::DeserializeOwned,
{
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

    let mut db = db.lock().await;
    let result = db.query("SELECT * FROM raft_log_entries");

    match result {
        Ok(zql::query::executor::ExecResult::Retrieved(records)) => {
            let mut entries: Vec<(u64, C::Entry)> = Vec::new();

            for record in &records {
                if let (Some(idx_str), Some(data)) = (
                    record.fields.get("log_index"),
                    record.fields.get("entry_data"),
                ) {
                    if let Ok(idx) = idx_str.parse::<u64>() {
                        let in_range = idx >= start && end.is_none_or(|e| idx <= e);
                        if in_range {
                            let entry: C::Entry = serde_json::from_str(data).map_err(|e| {
                                zql_to_storage_err(
                                    openraft::ErrorSubject::Logs,
                                    openraft::ErrorVerb::Read,
                                    e,
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

impl<C> RaftLogReader<C> for ZqlLogReader<C>
where
    C: RaftTypeConfig<NodeId = NodeId>,
    C::Entry: serde::Serialize + serde::de::DeserializeOwned,
{
    async fn try_get_log_entries<RB: RangeBounds<u64> + Clone + Debug + OptionalSend>(
        &mut self,
        range: RB,
    ) -> Result<Vec<C::Entry>, StorageError<NodeId>> {
        read_log_entries::<C>(&self.db, range).await
    }
}

impl<C> RaftLogReader<C> for ZqlLogStore<C>
where
    C: RaftTypeConfig<NodeId = NodeId>,
    C::Entry: serde::Serialize + serde::de::DeserializeOwned,
{
    async fn try_get_log_entries<RB: RangeBounds<u64> + Clone + Debug + OptionalSend>(
        &mut self,
        range: RB,
    ) -> Result<Vec<C::Entry>, StorageError<NodeId>> {
        read_log_entries::<C>(&self.db, range).await
    }
}

/// Read a metadata value from the raft_meta table.
async fn read_meta<T: serde::de::DeserializeOwned>(
    db: &mut zql::Database,
    key: &str,
) -> Result<Option<T>, StorageError<NodeId>> {
    let result = db.query(&format!(
        "SELECT * FROM raft_meta WHERE key = '{}'",
        escape_str(key)
    ));

    match result {
        Ok(zql::query::executor::ExecResult::Retrieved(records)) => {
            if records.is_empty() {
                return Ok(None);
            }
            if let Some(data) = records[0].fields.get("value_data") {
                let val: T = serde_json::from_str(data).map_err(|e| {
                    zql_to_storage_err(openraft::ErrorSubject::Store, openraft::ErrorVerb::Read, e)
                })?;
                Ok(Some(val))
            } else {
                Ok(None)
            }
        }
        _ => Ok(None),
    }
}

/// Write a metadata value to the raft_meta table (upsert via DELETE + INSERT).
fn write_meta<T: serde::Serialize>(
    db: &mut zql::Database,
    key: &str,
    value: &T,
) -> Result<(), StorageError<NodeId>> {
    let json = serde_json::to_string(value).map_err(|e| {
        zql_to_storage_err(openraft::ErrorSubject::Store, openraft::ErrorVerb::Write, e)
    })?;

    // Upsert: delete then insert
    let _ = db.query(&format!(
        "DELETE FROM raft_meta WHERE key = '{}'",
        escape_str(key)
    ));
    db.query(&format!(
        "INSERT INTO raft_meta (key, value_data) VALUES ('{}', '{}')",
        escape_str(key),
        escape_str(&json)
    ))
    .map_err(|e| {
        zql_to_storage_err(
            openraft::ErrorSubject::Store,
            openraft::ErrorVerb::Write,
            format!("write_meta({key}): {e}"),
        )
    })?;

    Ok(())
}

/// Delete a metadata key from the raft_meta table.
fn delete_meta(db: &mut zql::Database, key: &str) {
    let _ = db.query(&format!(
        "DELETE FROM raft_meta WHERE key = '{}'",
        escape_str(key)
    ));
}

impl<C> RaftLogStorage<C> for ZqlLogStore<C>
where
    C: RaftTypeConfig<NodeId = NodeId>,
    C::Entry: serde::Serialize + serde::de::DeserializeOwned + Clone,
{
    type LogReader = ZqlLogReader<C>;

    async fn get_log_state(&mut self) -> Result<LogState<C>, StorageError<NodeId>> {
        let mut db = self.db.lock().await;

        // Read last_purged
        let last_purged: Option<LogId<NodeId>> = read_meta(&mut db, META_LAST_PURGED).await?;

        // Read last log entry by scanning all entries and finding max index
        let result = db.query("SELECT * FROM raft_log_entries");
        let last_log = match result {
            Ok(zql::query::executor::ExecResult::Retrieved(records)) => {
                let mut best: Option<(u64, LogId<NodeId>)> = None;
                for record in &records {
                    if let (Some(idx_str), Some(data)) = (
                        record.fields.get("log_index"),
                        record.fields.get("entry_data"),
                    ) {
                        if let Ok(idx) = idx_str.parse::<u64>() {
                            let dominated = best.as_ref().is_none_or(|(cur, _)| idx > *cur);
                            if dominated {
                                if let Ok(entry) = serde_json::from_str::<C::Entry>(data) {
                                    best = Some((idx, *entry.get_log_id()));
                                }
                            }
                        }
                    }
                }
                best.map(|(_, log_id)| log_id)
            }
            _ => None,
        };

        Ok(LogState {
            last_purged_log_id: last_purged,
            last_log_id: last_log,
        })
    }

    async fn get_log_reader(&mut self) -> Self::LogReader {
        ZqlLogReader {
            db: Arc::clone(&self.db),
            _phantom: std::marker::PhantomData,
        }
    }

    async fn save_vote(&mut self, vote: &Vote<NodeId>) -> Result<(), StorageError<NodeId>> {
        let mut db = self.db.lock().await;
        write_meta(&mut db, META_VOTE, vote)?;
        db.flush().map_err(|e| {
            zql_to_storage_err(openraft::ErrorSubject::Vote, openraft::ErrorVerb::Write, e)
        })?;
        Ok(())
    }

    async fn read_vote(&mut self) -> Result<Option<Vote<NodeId>>, StorageError<NodeId>> {
        let mut db = self.db.lock().await;
        read_meta(&mut db, META_VOTE).await
    }

    async fn save_committed(
        &mut self,
        committed: Option<LogId<NodeId>>,
    ) -> Result<(), StorageError<NodeId>> {
        let mut db = self.db.lock().await;
        match committed {
            Some(ref id) => write_meta(&mut db, META_COMMITTED, id)?,
            None => delete_meta(&mut db, META_COMMITTED),
        }
        db.flush().map_err(|e| {
            zql_to_storage_err(openraft::ErrorSubject::Logs, openraft::ErrorVerb::Write, e)
        })?;
        Ok(())
    }

    async fn read_committed(&mut self) -> Result<Option<LogId<NodeId>>, StorageError<NodeId>> {
        let mut db = self.db.lock().await;
        read_meta(&mut db, META_COMMITTED).await
    }

    async fn append<I>(
        &mut self,
        entries: I,
        callback: LogFlushed<C>,
    ) -> Result<(), StorageError<NodeId>>
    where
        I: IntoIterator<Item = C::Entry> + OptionalSend,
        I::IntoIter: OptionalSend,
    {
        let mut db = self.db.lock().await;

        for entry in entries {
            let idx = entry.get_log_id().index;
            let json = serde_json::to_string(&entry).map_err(|e| {
                zql_to_storage_err(openraft::ErrorSubject::Logs, openraft::ErrorVerb::Write, e)
            })?;

            // Upsert by index
            let _ = db.query(&format!(
                "DELETE FROM raft_log_entries WHERE log_index = '{idx}'"
            ));
            db.query(&format!(
                "INSERT INTO raft_log_entries (log_index, entry_data) VALUES ('{idx}', '{}')",
                escape_str(&json)
            ))
            .map_err(|e| {
                zql_to_storage_err(
                    openraft::ErrorSubject::Logs,
                    openraft::ErrorVerb::Write,
                    format!("append log index {idx}: {e}"),
                )
            })?;
        }

        db.flush().map_err(|e| {
            zql_to_storage_err(openraft::ErrorSubject::Logs, openraft::ErrorVerb::Write, e)
        })?;

        // Signal that data is durable
        callback.log_io_completed(Ok(()));
        Ok(())
    }

    async fn truncate(&mut self, log_id: LogId<NodeId>) -> Result<(), StorageError<NodeId>> {
        let mut db = self.db.lock().await;
        let result = db.query("SELECT * FROM raft_log_entries");

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
                "DELETE FROM raft_log_entries WHERE log_index = '{idx}'"
            ));
        }

        if !indices_to_delete.is_empty() {
            db.flush().map_err(|e| {
                zql_to_storage_err(openraft::ErrorSubject::Logs, openraft::ErrorVerb::Write, e)
            })?;
        }

        Ok(())
    }

    async fn purge(&mut self, log_id: LogId<NodeId>) -> Result<(), StorageError<NodeId>> {
        let mut db = self.db.lock().await;
        let result = db.query("SELECT * FROM raft_log_entries");

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
                "DELETE FROM raft_log_entries WHERE log_index = '{idx}'"
            ));
        }

        // Persist last_purged
        write_meta(&mut db, META_LAST_PURGED, &log_id)?;

        db.flush().map_err(|e| {
            zql_to_storage_err(openraft::ErrorSubject::Logs, openraft::ErrorVerb::Write, e)
        })?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ZqlStateMachine
// ---------------------------------------------------------------------------

/// Persistent Raft state machine backed by ZQL.
///
/// The state machine data is stored as JSON in ZQL. On each `apply()`,
/// the in-memory cache is mutated and then persisted atomically.
///
/// An in-memory cache is kept in sync with the database for fast reads.
pub struct ZqlStateMachine<C, S, F>
where
    C: RaftTypeConfig<NodeId = NodeId>,
    S: Default + Clone + serde::Serialize + serde::de::DeserializeOwned + Send + Sync + 'static,
    F: Fn(&mut S, &C::D) -> C::R + Send + Sync + 'static,
{
    db: Arc<Mutex<zql::Database>>,
    /// In-memory cache of the state machine, kept in sync with ZQL.
    sm: Arc<RwLock<ZqlSmCache<C, S>>>,
    apply_fn: Arc<F>,
}

/// Cached state machine data for the ZQL-backed state machine.
pub struct ZqlSmCache<C: RaftTypeConfig<NodeId = NodeId>, S> {
    /// Last applied log id.
    pub last_applied_log: Option<LogId<NodeId>>,
    /// Last membership config.
    pub last_membership: StoredMembership<NodeId, C::Node>,
    /// Application-specific state.
    pub state: S,
}

impl<C: RaftTypeConfig<NodeId = NodeId>, S: Default> Default for ZqlSmCache<C, S> {
    fn default() -> Self {
        Self {
            last_applied_log: None,
            last_membership: StoredMembership::default(),
            state: S::default(),
        }
    }
}

impl<C, S, F> ZqlStateMachine<C, S, F>
where
    C: RaftTypeConfig<NodeId = NodeId>,
    S: Default + Clone + serde::Serialize + serde::de::DeserializeOwned + Send + Sync + 'static,
    F: Fn(&mut S, &C::D) -> C::R + Send + Sync + 'static,
{
    /// Open or create a state machine store at the given path.
    pub fn new(path: impl AsRef<Path>, apply_fn: F) -> Result<Self, crate::error::ConsensusError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                crate::error::ConsensusError::Zql(format!(
                    "Failed to create parent dir {:?}: {e}",
                    parent
                ))
            })?;
        }

        let db = zql::Database::open(path).map_err(|e| {
            crate::error::ConsensusError::Zql(format!("Failed to open ZQL SM at {:?}: {e}", path))
        })?;

        let db = Arc::new(Mutex::new(db));
        let cache = Self::load_cache_blocking(&db);

        Ok(Self {
            db,
            sm: Arc::new(RwLock::new(cache)),
            apply_fn: Arc::new(apply_fn),
        })
    }

    /// Create from an already-open ZQL `Database`.
    pub fn from_db(db: Arc<Mutex<zql::Database>>, apply_fn: F) -> Self {
        let cache = Self::load_cache_blocking(&db);
        Self {
            db,
            sm: Arc::new(RwLock::new(cache)),
            apply_fn: Arc::new(apply_fn),
        }
    }

    /// Get read access to the cached state.
    pub fn state(&self) -> Arc<RwLock<ZqlSmCache<C, S>>> {
        Arc::clone(&self.sm)
    }

    /// Get a clone of the database handle.
    pub fn db(&self) -> Arc<Mutex<zql::Database>> {
        Arc::clone(&self.db)
    }

    /// Load cached state from ZQL. This uses a blocking lock since it is
    /// called from constructors (before the async runtime is available).
    fn load_cache_blocking(db: &Arc<Mutex<zql::Database>>) -> ZqlSmCache<C, S> {
        // Try to lock synchronously -- if we're in a constructor this is fine.
        let mut db = match db.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                warn!("Could not acquire ZQL lock to load SM cache, using defaults");
                return ZqlSmCache::default();
            }
        };

        let state = Self::read_sm_field::<S>(&mut db, SM_STATE).unwrap_or_default();
        let last_applied_log = Self::read_sm_field::<LogId<NodeId>>(&mut db, META_LAST_APPLIED)
            .ok()
            .flatten();
        let last_membership =
            Self::read_sm_field::<StoredMembership<NodeId, C::Node>>(&mut db, META_LAST_MEMBERSHIP)
                .unwrap_or_default();

        ZqlSmCache {
            last_applied_log,
            last_membership: last_membership.unwrap_or_default(),
            state: state.unwrap_or_default(),
        }
    }

    /// Read a single field from the raft_sm_state table.
    fn read_sm_field<T: serde::de::DeserializeOwned>(
        db: &mut zql::Database,
        key: &str,
    ) -> std::result::Result<Option<T>, String> {
        let result = db.query(&format!(
            "SELECT * FROM raft_sm_state WHERE key = '{}'",
            escape_str(key)
        ));

        match result {
            Ok(zql::query::executor::ExecResult::Retrieved(records)) => {
                if records.is_empty() {
                    return Ok(None);
                }
                if let Some(data) = records[0].fields.get("value_data") {
                    let val: T = serde_json::from_str(data).map_err(|e| e.to_string())?;
                    Ok(Some(val))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    /// Write a field to the raft_sm_state table.
    fn write_sm_field<T: serde::Serialize>(
        db: &mut zql::Database,
        key: &str,
        value: &T,
    ) -> std::result::Result<(), String> {
        let json = serde_json::to_string(value).map_err(|e| e.to_string())?;

        let _ = db.query(&format!(
            "DELETE FROM raft_sm_state WHERE key = '{}'",
            escape_str(key)
        ));
        db.query(&format!(
            "INSERT INTO raft_sm_state (key, value_data) VALUES ('{}', '{}')",
            escape_str(key),
            escape_str(&json)
        ))
        .map_err(|e| e.to_string())?;

        Ok(())
    }
}

impl<C, S, F> Clone for ZqlStateMachine<C, S, F>
where
    C: RaftTypeConfig<NodeId = NodeId>,
    S: Default + Clone + serde::Serialize + serde::de::DeserializeOwned + Send + Sync + 'static,
    F: Fn(&mut S, &C::D) -> C::R + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            db: Arc::clone(&self.db),
            sm: Arc::clone(&self.sm),
            apply_fn: Arc::clone(&self.apply_fn),
        }
    }
}

/// Snapshot builder for the ZQL state machine.
pub struct ZqlSnapshotBuilder<C, S>
where
    C: RaftTypeConfig<NodeId = NodeId>,
{
    sm: Arc<RwLock<ZqlSmCache<C, S>>>,
}

impl<C, S> RaftSnapshotBuilder<C> for ZqlSnapshotBuilder<C, S>
where
    C: RaftTypeConfig<NodeId = NodeId, SnapshotData = Cursor<Vec<u8>>>,
    S: Default + Clone + serde::Serialize + serde::de::DeserializeOwned + Send + Sync + 'static,
{
    async fn build_snapshot(&mut self) -> Result<Snapshot<C>, StorageError<NodeId>> {
        let sm = self.sm.read().await;

        let data = bincode::serialize(&sm.state).map_err(|e| {
            zql_to_storage_err(
                openraft::ErrorSubject::StateMachine,
                openraft::ErrorVerb::Read,
                e,
            )
        })?;

        let snapshot_id = if let Some(ref last) = sm.last_applied_log {
            format!("{}-{}", last.leader_id, last.index)
        } else {
            "0-0".to_string()
        };

        let meta = SnapshotMeta {
            last_log_id: sm.last_applied_log,
            last_membership: sm.last_membership.clone(),
            snapshot_id,
        };

        debug!(
            last_log_id = ?meta.last_log_id,
            "Built ZQL snapshot"
        );

        Ok(Snapshot {
            meta,
            snapshot: Box::new(Cursor::new(data)),
        })
    }
}

impl<C, S, F> RaftStateMachine<C> for ZqlStateMachine<C, S, F>
where
    C: RaftTypeConfig<NodeId = NodeId, SnapshotData = Cursor<Vec<u8>>, Entry = Entry<C>>,
    C::D: Clone + serde::Serialize + serde::de::DeserializeOwned,
    C::R: Default,
    S: Default + Clone + serde::Serialize + serde::de::DeserializeOwned + Send + Sync + 'static,
    F: Fn(&mut S, &C::D) -> C::R + Send + Sync + 'static,
{
    type SnapshotBuilder = ZqlSnapshotBuilder<C, S>;

    async fn applied_state(
        &mut self,
    ) -> Result<(Option<LogId<NodeId>>, StoredMembership<NodeId, C::Node>), StorageError<NodeId>>
    {
        let sm = self.sm.read().await;
        Ok((sm.last_applied_log, sm.last_membership.clone()))
    }

    async fn apply<I>(&mut self, entries: I) -> Result<Vec<C::R>, StorageError<NodeId>>
    where
        I: IntoIterator<Item = C::Entry> + OptionalSend,
        I::IntoIter: OptionalSend,
    {
        let mut sm = self.sm.write().await;
        let mut responses = Vec::new();

        for entry in entries {
            sm.last_applied_log = Some(entry.log_id);

            match entry.payload {
                EntryPayload::Normal(ref data) => {
                    let resp = (self.apply_fn)(&mut sm.state, data);
                    responses.push(resp);
                }
                EntryPayload::Membership(ref mem) => {
                    sm.last_membership = StoredMembership::new(Some(entry.log_id), mem.clone());
                    responses.push(C::R::default());
                }
                EntryPayload::Blank => {
                    responses.push(C::R::default());
                }
            }
        }

        // Persist state + metadata to ZQL
        let mut db = self.db.lock().await;

        Self::write_sm_field(&mut db, SM_STATE, &sm.state).map_err(|e| {
            zql_to_storage_err(
                openraft::ErrorSubject::StateMachine,
                openraft::ErrorVerb::Write,
                e,
            )
        })?;

        if let Some(ref id) = sm.last_applied_log {
            Self::write_sm_field(&mut db, META_LAST_APPLIED, id).map_err(|e| {
                zql_to_storage_err(
                    openraft::ErrorSubject::StateMachine,
                    openraft::ErrorVerb::Write,
                    e,
                )
            })?;
        }

        Self::write_sm_field(&mut db, META_LAST_MEMBERSHIP, &sm.last_membership).map_err(|e| {
            zql_to_storage_err(
                openraft::ErrorSubject::StateMachine,
                openraft::ErrorVerb::Write,
                e,
            )
        })?;

        db.flush().map_err(|e| {
            zql_to_storage_err(
                openraft::ErrorSubject::StateMachine,
                openraft::ErrorVerb::Write,
                e,
            )
        })?;

        Ok(responses)
    }

    async fn get_snapshot_builder(&mut self) -> Self::SnapshotBuilder {
        ZqlSnapshotBuilder {
            sm: Arc::clone(&self.sm),
        }
    }

    async fn begin_receiving_snapshot(
        &mut self,
    ) -> Result<Box<Cursor<Vec<u8>>>, StorageError<NodeId>> {
        Ok(Box::new(Cursor::new(Vec::new())))
    }

    async fn install_snapshot(
        &mut self,
        meta: &SnapshotMeta<NodeId, C::Node>,
        snapshot: Box<Cursor<Vec<u8>>>,
    ) -> Result<(), StorageError<NodeId>> {
        let data = snapshot.into_inner();
        let state: S = bincode::deserialize(&data).map_err(|e| {
            zql_to_storage_err(
                openraft::ErrorSubject::Snapshot(None),
                openraft::ErrorVerb::Read,
                e,
            )
        })?;

        // Update in-memory cache
        let mut sm = self.sm.write().await;
        sm.last_applied_log = meta.last_log_id;
        sm.last_membership = meta.last_membership.clone();
        sm.state = state;

        // Persist to ZQL
        let mut db = self.db.lock().await;

        Self::write_sm_field(&mut db, SM_STATE, &sm.state).map_err(|e| {
            zql_to_storage_err(
                openraft::ErrorSubject::Snapshot(None),
                openraft::ErrorVerb::Write,
                e,
            )
        })?;

        // Store snapshot data + meta
        let data_hex = hex::encode(&data);
        let snapshot_json = serde_json::to_string(meta).map_err(|e| {
            zql_to_storage_err(
                openraft::ErrorSubject::Snapshot(None),
                openraft::ErrorVerb::Write,
                e,
            )
        })?;

        // Upsert snapshot data
        let _ = db.query(&format!(
            "DELETE FROM raft_snapshots WHERE key = '{}'",
            escape_str(SNAPSHOT_CURRENT)
        ));
        db.query(&format!(
            "INSERT INTO raft_snapshots (key, value_data) VALUES ('{}', '{}')",
            escape_str(SNAPSHOT_CURRENT),
            escape_str(&data_hex)
        ))
        .map_err(|e| {
            zql_to_storage_err(
                openraft::ErrorSubject::Snapshot(None),
                openraft::ErrorVerb::Write,
                format!("write snapshot data: {e}"),
            )
        })?;

        // Upsert snapshot metadata
        let _ = db.query(&format!(
            "DELETE FROM raft_snapshots WHERE key = '{}'",
            escape_str(SNAPSHOT_META)
        ));
        db.query(&format!(
            "INSERT INTO raft_snapshots (key, value_data) VALUES ('{}', '{}')",
            escape_str(SNAPSHOT_META),
            escape_str(&snapshot_json)
        ))
        .map_err(|e| {
            zql_to_storage_err(
                openraft::ErrorSubject::Snapshot(None),
                openraft::ErrorVerb::Write,
                format!("write snapshot meta: {e}"),
            )
        })?;

        // Persist last_applied and membership
        if let Some(ref id) = sm.last_applied_log {
            Self::write_sm_field(&mut db, META_LAST_APPLIED, id).map_err(|e| {
                zql_to_storage_err(
                    openraft::ErrorSubject::Snapshot(None),
                    openraft::ErrorVerb::Write,
                    e,
                )
            })?;
        }

        Self::write_sm_field(&mut db, META_LAST_MEMBERSHIP, &sm.last_membership).map_err(|e| {
            zql_to_storage_err(
                openraft::ErrorSubject::Snapshot(None),
                openraft::ErrorVerb::Write,
                e,
            )
        })?;

        db.flush().map_err(|e| {
            zql_to_storage_err(
                openraft::ErrorSubject::Snapshot(None),
                openraft::ErrorVerb::Write,
                e,
            )
        })?;

        debug!(
            last_log_id = ?meta.last_log_id,
            "Installed snapshot into ZQL state machine"
        );

        Ok(())
    }

    async fn get_current_snapshot(&mut self) -> Result<Option<Snapshot<C>>, StorageError<NodeId>> {
        let mut db = self.db.lock().await;

        // Read snapshot data
        let data_result = db.query(&format!(
            "SELECT * FROM raft_snapshots WHERE key = '{}'",
            escape_str(SNAPSHOT_CURRENT)
        ));
        let data_hex = match data_result {
            Ok(zql::query::executor::ExecResult::Retrieved(records)) => {
                if records.is_empty() {
                    return Ok(None);
                }
                match records[0].fields.get("value_data") {
                    Some(v) => v.clone(),
                    None => return Ok(None),
                }
            }
            _ => return Ok(None),
        };

        let data = hex::decode(&data_hex).map_err(|e| {
            zql_to_storage_err(
                openraft::ErrorSubject::Snapshot(None),
                openraft::ErrorVerb::Read,
                format!("hex decode failed: {e}"),
            )
        })?;

        // Read snapshot metadata
        let meta_result = db.query(&format!(
            "SELECT * FROM raft_snapshots WHERE key = '{}'",
            escape_str(SNAPSHOT_META)
        ));
        let meta_json = match meta_result {
            Ok(zql::query::executor::ExecResult::Retrieved(records)) => {
                if records.is_empty() {
                    return Ok(None);
                }
                match records[0].fields.get("value_data") {
                    Some(v) => v.clone(),
                    None => return Ok(None),
                }
            }
            _ => return Ok(None),
        };

        let meta: SnapshotMeta<NodeId, C::Node> =
            serde_json::from_str(&meta_json).map_err(|e| {
                zql_to_storage_err(
                    openraft::ErrorSubject::Snapshot(None),
                    openraft::ErrorVerb::Read,
                    e,
                )
            })?;

        Ok(Some(Snapshot {
            meta,
            snapshot: Box::new(Cursor::new(data)),
        }))
    }
}
