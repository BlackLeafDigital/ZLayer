//! In-memory storage implementation using the openraft v2 storage API.
//!
//! This store keeps all Raft log entries, votes, and state machine snapshots
//! in memory using `BTreeMap` and `RwLock`. It is generic over the
//! `RaftTypeConfig` so any application can plug in its own request/response types.
//!
//! **Not suitable for production** -- data is lost on process restart.
//! Use [`RedbStore`](super::redb_store) for durable storage.

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::io::Cursor;
use std::ops::RangeBounds;
use std::sync::Arc;

use openraft::log_id::RaftLogId;
use openraft::storage::{LogFlushed, LogState, RaftLogStorage, RaftStateMachine, Snapshot};
use openraft::{
    Entry, EntryPayload, LogId, OptionalSend, RaftLogReader, RaftSnapshotBuilder, RaftTypeConfig,
    SnapshotMeta, StorageError, StoredMembership, Vote,
};
use tokio::sync::RwLock;
use tracing::debug;

use crate::types::NodeId;

// ---------------------------------------------------------------------------
// Log reader (clone-able handle used by replication tasks)
// ---------------------------------------------------------------------------

/// A cloneable log reader for the in-memory store.
pub struct MemLogReader<C: RaftTypeConfig<NodeId = NodeId>> {
    log: Arc<RwLock<LogData<C>>>,
}

impl<C: RaftTypeConfig<NodeId = NodeId>> Clone for MemLogReader<C> {
    fn clone(&self) -> Self {
        Self {
            log: Arc::clone(&self.log),
        }
    }
}

impl<C> RaftLogReader<C> for MemLogReader<C>
where
    C: RaftTypeConfig<NodeId = NodeId>,
    C::Entry: Clone,
{
    async fn try_get_log_entries<RB: RangeBounds<u64> + Clone + Debug + OptionalSend>(
        &mut self,
        range: RB,
    ) -> Result<Vec<C::Entry>, StorageError<NodeId>> {
        let log = self.log.read().await;
        let entries = log.entries.range(range).map(|(_, e)| e.clone()).collect();
        Ok(entries)
    }
}

// ---------------------------------------------------------------------------
// Internal data structs
// ---------------------------------------------------------------------------

/// Internal log data protected by a `RwLock`.
struct LogData<C: RaftTypeConfig<NodeId = NodeId>> {
    last_purged_log_id: Option<LogId<NodeId>>,
    entries: BTreeMap<u64, C::Entry>,
    vote: Option<Vote<NodeId>>,
    committed: Option<LogId<NodeId>>,
}

impl<C: RaftTypeConfig<NodeId = NodeId>> Default for LogData<C> {
    fn default() -> Self {
        Self {
            last_purged_log_id: None,
            entries: BTreeMap::new(),
            vote: None,
            committed: None,
        }
    }
}

/// Internal state-machine data protected by a `RwLock`.
///
/// The generic parameter `S` is the application's state type. It must be
/// `Default + Clone + serde::Serialize + serde::de::DeserializeOwned`.
pub struct SmData<C: RaftTypeConfig<NodeId = NodeId>, S> {
    /// Last applied log id.
    pub last_applied_log: Option<LogId<NodeId>>,
    /// Last membership config.
    pub last_membership: StoredMembership<NodeId, C::Node>,
    /// Application-specific state.
    pub state: S,
    /// Current snapshot bytes (serialized `S`), if any.
    pub current_snapshot: Option<StoredSnapshot<C>>,
}

impl<C: RaftTypeConfig<NodeId = NodeId>, S: Default> Default for SmData<C, S> {
    fn default() -> Self {
        Self {
            last_applied_log: None,
            last_membership: StoredMembership::default(),
            state: S::default(),
            current_snapshot: None,
        }
    }
}

/// A serialized snapshot stored in memory.
pub struct StoredSnapshot<C: RaftTypeConfig> {
    pub meta: SnapshotMeta<C::NodeId, C::Node>,
    pub data: Vec<u8>,
}

impl<C: RaftTypeConfig> Clone for StoredSnapshot<C> {
    fn clone(&self) -> Self {
        Self {
            meta: self.meta.clone(),
            data: self.data.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// MemLogStore -- implements RaftLogStorage
// ---------------------------------------------------------------------------

/// In-memory Raft log store (v2 API).
///
/// Implements `RaftLogReader` and `RaftLogStorage`.
pub struct MemLogStore<C: RaftTypeConfig<NodeId = NodeId>> {
    log: Arc<RwLock<LogData<C>>>,
}

impl<C: RaftTypeConfig<NodeId = NodeId>> MemLogStore<C> {
    /// Create a new, empty log store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            log: Arc::new(RwLock::new(LogData::default())),
        }
    }
}

impl<C: RaftTypeConfig<NodeId = NodeId>> Default for MemLogStore<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C: RaftTypeConfig<NodeId = NodeId>> Clone for MemLogStore<C> {
    fn clone(&self) -> Self {
        Self {
            log: Arc::clone(&self.log),
        }
    }
}

impl<C> RaftLogReader<C> for MemLogStore<C>
where
    C: RaftTypeConfig<NodeId = NodeId>,
    C::Entry: Clone,
{
    async fn try_get_log_entries<RB: RangeBounds<u64> + Clone + Debug + OptionalSend>(
        &mut self,
        range: RB,
    ) -> Result<Vec<C::Entry>, StorageError<NodeId>> {
        let log = self.log.read().await;
        let entries = log.entries.range(range).map(|(_, e)| e.clone()).collect();
        Ok(entries)
    }
}

impl<C> RaftLogStorage<C> for MemLogStore<C>
where
    C: RaftTypeConfig<NodeId = NodeId>,
    C::Entry: Clone,
{
    type LogReader = MemLogReader<C>;

    async fn get_log_state(&mut self) -> Result<LogState<C>, StorageError<NodeId>> {
        let log = self.log.read().await;
        let last = log
            .entries
            .iter()
            .next_back()
            .map(|(_, ent)| *ent.get_log_id());

        Ok(LogState {
            last_purged_log_id: log.last_purged_log_id,
            last_log_id: last,
        })
    }

    async fn get_log_reader(&mut self) -> Self::LogReader {
        MemLogReader {
            log: Arc::clone(&self.log),
        }
    }

    async fn save_vote(&mut self, vote: &Vote<NodeId>) -> Result<(), StorageError<NodeId>> {
        let mut log = self.log.write().await;
        log.vote = Some(*vote);
        Ok(())
    }

    async fn read_vote(&mut self) -> Result<Option<Vote<NodeId>>, StorageError<NodeId>> {
        let log = self.log.read().await;
        Ok(log.vote)
    }

    async fn save_committed(
        &mut self,
        committed: Option<LogId<NodeId>>,
    ) -> Result<(), StorageError<NodeId>> {
        let mut log = self.log.write().await;
        log.committed = committed;
        Ok(())
    }

    async fn read_committed(&mut self) -> Result<Option<LogId<NodeId>>, StorageError<NodeId>> {
        let log = self.log.read().await;
        Ok(log.committed)
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
        let mut log = self.log.write().await;
        for entry in entries {
            let idx = entry.get_log_id().index;
            log.entries.insert(idx, entry);
        }
        // In-memory store: data is immediately "flushed".
        callback.log_io_completed(Ok(()));
        Ok(())
    }

    async fn truncate(&mut self, log_id: LogId<NodeId>) -> Result<(), StorageError<NodeId>> {
        let mut log = self.log.write().await;
        let keys: Vec<u64> = log.entries.range(log_id.index..).map(|(k, _)| *k).collect();
        for key in keys {
            log.entries.remove(&key);
        }
        Ok(())
    }

    async fn purge(&mut self, log_id: LogId<NodeId>) -> Result<(), StorageError<NodeId>> {
        let mut log = self.log.write().await;
        let keys: Vec<u64> = log
            .entries
            .range(..=log_id.index)
            .map(|(k, _)| *k)
            .collect();
        for key in keys {
            log.entries.remove(&key);
        }
        log.last_purged_log_id = Some(log_id);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// MemStateMachine -- implements RaftStateMachine
// ---------------------------------------------------------------------------

/// In-memory Raft state machine (v2 API).
///
/// Generic over:
/// - `C`: openraft `RaftTypeConfig`
/// - `S`: application state type
/// - `F`: a function `fn(&mut S, &C::D) -> C::R` that applies a log entry to the state.
///
/// The `apply_fn` approach keeps the state machine generic: the application provides
/// a closure that knows how to mutate `S` given a log entry payload of type `C::D`.
pub struct MemStateMachine<C, S, F>
where
    C: RaftTypeConfig<NodeId = NodeId>,
    S: Default + Clone + serde::Serialize + serde::de::DeserializeOwned + Send + Sync + 'static,
    F: Fn(&mut S, &C::D) -> C::R + Send + Sync + 'static,
{
    sm: Arc<RwLock<SmData<C, S>>>,
    apply_fn: Arc<F>,
}

impl<C, S, F> MemStateMachine<C, S, F>
where
    C: RaftTypeConfig<NodeId = NodeId>,
    S: Default + Clone + serde::Serialize + serde::de::DeserializeOwned + Send + Sync + 'static,
    F: Fn(&mut S, &C::D) -> C::R + Send + Sync + 'static,
{
    /// Create a new state machine with the given apply function.
    pub fn new(apply_fn: F) -> Self {
        Self {
            sm: Arc::new(RwLock::new(SmData::default())),
            apply_fn: Arc::new(apply_fn),
        }
    }

    /// Get a read handle to the inner state machine data (for reading application state).
    #[must_use]
    pub fn data(&self) -> Arc<RwLock<SmData<C, S>>> {
        Arc::clone(&self.sm)
    }
}

impl<C, S, F> Clone for MemStateMachine<C, S, F>
where
    C: RaftTypeConfig<NodeId = NodeId>,
    S: Default + Clone + serde::Serialize + serde::de::DeserializeOwned + Send + Sync + 'static,
    F: Fn(&mut S, &C::D) -> C::R + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            sm: Arc::clone(&self.sm),
            apply_fn: Arc::clone(&self.apply_fn),
        }
    }
}

// -- Snapshot builder -------------------------------------------------------

/// Snapshot builder for the in-memory state machine.
pub struct MemSnapshotBuilder<C, S, F>
where
    C: RaftTypeConfig<NodeId = NodeId>,
    S: Default + Clone + serde::Serialize + serde::de::DeserializeOwned + Send + Sync + 'static,
    F: Fn(&mut S, &C::D) -> C::R + Send + Sync + 'static,
{
    sm: Arc<RwLock<SmData<C, S>>>,
    _phantom: std::marker::PhantomData<F>,
}

impl<C, S, F> RaftSnapshotBuilder<C> for MemSnapshotBuilder<C, S, F>
where
    C: RaftTypeConfig<NodeId = NodeId, SnapshotData = Cursor<Vec<u8>>>,
    S: Default + Clone + serde::Serialize + serde::de::DeserializeOwned + Send + Sync + 'static,
    F: Fn(&mut S, &C::D) -> C::R + Send + Sync + 'static,
{
    async fn build_snapshot(&mut self) -> Result<Snapshot<C>, StorageError<NodeId>> {
        let sm = self.sm.read().await;

        let data = bincode::serialize(&sm.state).map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::StateMachine,
                openraft::ErrorVerb::Read,
                std::io::Error::other(e),
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
            "Built in-memory snapshot"
        );

        Ok(Snapshot {
            meta,
            snapshot: Box::new(Cursor::new(data)),
        })
    }
}

// -- RaftStateMachine impl --------------------------------------------------

impl<C, S, F> RaftStateMachine<C> for MemStateMachine<C, S, F>
where
    C: RaftTypeConfig<NodeId = NodeId, SnapshotData = Cursor<Vec<u8>>, Entry = Entry<C>>,
    C::D: Clone + serde::Serialize + serde::de::DeserializeOwned,
    C::R: Default,
    S: Default + Clone + serde::Serialize + serde::de::DeserializeOwned + Send + Sync + 'static,
    F: Fn(&mut S, &C::D) -> C::R + Send + Sync + 'static,
{
    type SnapshotBuilder = MemSnapshotBuilder<C, S, F>;

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

        Ok(responses)
    }

    async fn get_snapshot_builder(&mut self) -> Self::SnapshotBuilder {
        MemSnapshotBuilder {
            sm: Arc::clone(&self.sm),
            _phantom: std::marker::PhantomData,
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
            StorageError::from_io_error(
                openraft::ErrorSubject::Snapshot(None),
                openraft::ErrorVerb::Read,
                std::io::Error::other(e),
            )
        })?;

        let mut sm = self.sm.write().await;
        sm.last_applied_log = meta.last_log_id;
        sm.last_membership = meta.last_membership.clone();
        sm.state = state.clone();

        // Store the snapshot
        let snapshot_data = bincode::serialize(&state).map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::Snapshot(None),
                openraft::ErrorVerb::Write,
                std::io::Error::other(e),
            )
        })?;
        sm.current_snapshot = Some(StoredSnapshot {
            meta: meta.clone(),
            data: snapshot_data,
        });

        debug!(
            last_log_id = ?meta.last_log_id,
            "Installed snapshot into in-memory state machine"
        );

        Ok(())
    }

    async fn get_current_snapshot(&mut self) -> Result<Option<Snapshot<C>>, StorageError<NodeId>> {
        let sm = self.sm.read().await;
        match &sm.current_snapshot {
            Some(stored) => Ok(Some(Snapshot {
                meta: stored.meta.clone(),
                snapshot: Box::new(Cursor::new(stored.data.clone())),
            })),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openraft::storage::RaftLogStorage;

    // Minimal type config for testing
    openraft::declare_raft_types!(
        pub TestTypeConfig:
            D = String,
            R = String,
    );

    #[tokio::test]
    async fn test_log_store_empty_state() {
        let mut store = MemLogStore::<TestTypeConfig>::new();
        let state = RaftLogStorage::get_log_state(&mut store).await.unwrap();
        assert!(state.last_purged_log_id.is_none());
        assert!(state.last_log_id.is_none());
    }

    #[tokio::test]
    async fn test_vote_round_trip() {
        let mut store = MemLogStore::<TestTypeConfig>::new();

        let vote = RaftLogStorage::read_vote(&mut store).await.unwrap();
        assert!(vote.is_none());

        let new_vote = Vote::new(1, 1);
        RaftLogStorage::save_vote(&mut store, &new_vote)
            .await
            .unwrap();

        let vote = RaftLogStorage::read_vote(&mut store).await.unwrap();
        assert_eq!(vote, Some(new_vote));
    }

    #[tokio::test]
    async fn test_state_machine_new() {
        let sm = MemStateMachine::<TestTypeConfig, Vec<String>, _>::new(
            |state: &mut Vec<String>, data: &String| {
                state.push(data.clone());
                format!("applied: {data}")
            },
        );

        let data = sm.data();
        {
            let d = data.read().await;
            assert!(d.state.is_empty());
        }
    }
}
