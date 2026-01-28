//! In-memory storage implementation for OpenRaft
//!
//! Provides log storage and state machine for the scheduler's Raft consensus.
//! Uses the RaftStorage v1 API which combines log and state machine storage.

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::io::Cursor;
use std::ops::RangeBounds;
use std::sync::Arc;

use openraft::storage::{LogState, RaftStorage, Snapshot};
use openraft::{
    Entry, EntryPayload, LogId, OptionalSend, RaftLogReader, RaftSnapshotBuilder, SnapshotMeta,
    StorageError, StoredMembership, Vote,
};
use tokio::sync::RwLock;

use crate::raft::{ClusterState, NodeId, Response, TypeConfig};

/// In-memory log storage
#[derive(Debug, Default)]
pub struct LogStore {
    /// Last purged log ID
    last_purged_log_id: Option<LogId<NodeId>>,
    /// Log entries indexed by log index
    log: BTreeMap<u64, Entry<TypeConfig>>,
    /// Current vote
    vote: Option<Vote<NodeId>>,
}

/// In-memory state machine
#[derive(Debug, Default)]
pub struct StateMachine {
    /// Last applied log entry
    pub last_applied_log: Option<LogId<NodeId>>,
    /// Last membership config
    pub last_membership: StoredMembership<NodeId, openraft::BasicNode>,
    /// The actual cluster state
    pub state: ClusterState,
}

/// Combined in-memory storage for OpenRaft (v1 API)
///
/// This implements the unified `RaftStorage` trait which combines
/// log storage and state machine operations.
pub struct MemStore {
    log: Arc<RwLock<LogStore>>,
    sm: Arc<RwLock<StateMachine>>,
}

impl MemStore {
    /// Create a new in-memory store
    pub fn new() -> Self {
        Self {
            log: Arc::new(RwLock::new(LogStore::default())),
            sm: Arc::new(RwLock::new(StateMachine::default())),
        }
    }

    /// Get a reference to the state machine for reading cluster state
    pub fn state_machine(&self) -> Arc<RwLock<StateMachine>> {
        Arc::clone(&self.sm)
    }

    /// Get a reference to the log store
    pub fn log_store(&self) -> Arc<RwLock<LogStore>> {
        Arc::clone(&self.log)
    }
}

impl Default for MemStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for MemStore {
    fn clone(&self) -> Self {
        Self {
            log: Arc::clone(&self.log),
            sm: Arc::clone(&self.sm),
        }
    }
}

impl Debug for MemStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemStore").finish()
    }
}

// Implement RaftLogReader for reading log entries
impl RaftLogReader<TypeConfig> for MemStore {
    async fn try_get_log_entries<RB: RangeBounds<u64> + Clone + Debug + OptionalSend>(
        &mut self,
        range: RB,
    ) -> Result<Vec<Entry<TypeConfig>>, StorageError<NodeId>> {
        let log = self.log.read().await;
        let response = log
            .log
            .range(range)
            .map(|(_, val)| val.clone())
            .collect::<Vec<_>>();
        Ok(response)
    }
}

// Implement RaftSnapshotBuilder for creating snapshots
impl RaftSnapshotBuilder<TypeConfig> for MemStore {
    async fn build_snapshot(&mut self) -> Result<Snapshot<TypeConfig>, StorageError<NodeId>> {
        let sm = self.sm.read().await;

        let data = serde_json::to_vec(&sm.state).map_err(|e| {
            StorageError::from_io_error(
                openraft::ErrorSubject::StateMachine,
                openraft::ErrorVerb::Read,
                std::io::Error::other(e),
            )
        })?;

        let last_applied_log = sm.last_applied_log;
        let last_membership = sm.last_membership.clone();

        let snapshot_id = if let Some(last) = last_applied_log {
            format!("{}-{}", last.leader_id, last.index)
        } else {
            "0-0".to_string()
        };

        let meta = SnapshotMeta {
            last_log_id: last_applied_log,
            last_membership,
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
impl RaftStorage<TypeConfig> for MemStore {
    type LogReader = Self;
    type SnapshotBuilder = Self;

    // === Vote operations ===

    async fn save_vote(&mut self, vote: &Vote<NodeId>) -> Result<(), StorageError<NodeId>> {
        let mut log = self.log.write().await;
        log.vote = Some(*vote);
        Ok(())
    }

    async fn read_vote(&mut self) -> Result<Option<Vote<NodeId>>, StorageError<NodeId>> {
        let log = self.log.read().await;
        Ok(log.vote)
    }

    // === Log operations ===

    async fn get_log_state(&mut self) -> Result<LogState<TypeConfig>, StorageError<NodeId>> {
        let log = self.log.read().await;
        let last_purged = log.last_purged_log_id;
        let last = log.log.iter().next_back().map(|(_, ent)| ent.log_id);

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
        let mut log = self.log.write().await;
        for entry in entries {
            log.log.insert(entry.log_id.index, entry);
        }
        Ok(())
    }

    async fn delete_conflict_logs_since(
        &mut self,
        log_id: LogId<NodeId>,
    ) -> Result<(), StorageError<NodeId>> {
        let mut log = self.log.write().await;
        let keys: Vec<u64> = log.log.range(log_id.index..).map(|(k, _)| *k).collect();
        for key in keys {
            log.log.remove(&key);
        }
        Ok(())
    }

    async fn purge_logs_upto(&mut self, log_id: LogId<NodeId>) -> Result<(), StorageError<NodeId>> {
        let mut log = self.log.write().await;

        let keys: Vec<u64> = log.log.range(..=log_id.index).map(|(k, _)| *k).collect();
        for key in keys {
            log.log.remove(&key);
        }

        log.last_purged_log_id = Some(log_id);
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
        let sm = self.sm.read().await;
        Ok((sm.last_applied_log, sm.last_membership.clone()))
    }

    async fn apply_to_state_machine(
        &mut self,
        entries: &[Entry<TypeConfig>],
    ) -> Result<Vec<Response>, StorageError<NodeId>> {
        let mut sm = self.sm.write().await;
        let mut responses = Vec::new();

        for entry in entries {
            sm.last_applied_log = Some(entry.log_id);

            match &entry.payload {
                EntryPayload::Blank => {
                    responses.push(Response::Success { data: None });
                }
                EntryPayload::Normal(req) => {
                    let resp = sm.state.apply(req);
                    responses.push(resp);
                }
                EntryPayload::Membership(mem) => {
                    sm.last_membership = StoredMembership::new(Some(entry.log_id), mem.clone());
                    responses.push(Response::Success { data: None });
                }
            }
        }

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

        let mut sm = self.sm.write().await;
        sm.last_applied_log = meta.last_log_id;
        sm.last_membership = meta.last_membership.clone();
        sm.state = state;

        Ok(())
    }

    async fn get_current_snapshot(
        &mut self,
    ) -> Result<Option<Snapshot<TypeConfig>>, StorageError<NodeId>> {
        // For simplicity, always build a fresh snapshot when needed
        // In production, you'd cache snapshots
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raft::{Request, ServiceState};

    #[tokio::test]
    async fn test_mem_store_log_operations() {
        let mut store = MemStore::new();

        // Check initial state
        let log_state = RaftStorage::get_log_state(&mut store).await.unwrap();
        assert!(log_state.last_log_id.is_none());
        assert!(log_state.last_purged_log_id.is_none());
    }

    #[tokio::test]
    async fn test_vote_operations() {
        let mut store = MemStore::new();

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
    async fn test_state_machine_apply() {
        let store = MemStore::new();
        let sm = store.state_machine();

        // Apply a service state update directly to the state
        {
            let mut sm = sm.write().await;
            sm.state.apply(&Request::UpdateServiceState {
                service_name: "test".to_string(),
                state: ServiceState {
                    current_replicas: 3,
                    ..Default::default()
                },
            });
        }

        // Verify the state
        {
            let sm = sm.read().await;
            let svc = sm.state.get_service("test").unwrap();
            assert_eq!(svc.current_replicas, 3);
        }
    }

    #[tokio::test]
    async fn test_log_append_and_read() {
        use openraft::CommittedLeaderId;

        let mut store = MemStore::new();

        // Create a CommittedLeaderId and LogId properly
        let leader_id = CommittedLeaderId::new(1, 1);
        let log_id = LogId::new(leader_id, 1);

        // Create a log entry
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
    async fn test_snapshot_build() {
        let mut store = MemStore::new();

        // Build a snapshot from empty state
        let snapshot = RaftSnapshotBuilder::build_snapshot(&mut store)
            .await
            .unwrap();

        assert!(snapshot.meta.last_log_id.is_none());
        assert_eq!(snapshot.meta.snapshot_id, "0-0");
    }
}
