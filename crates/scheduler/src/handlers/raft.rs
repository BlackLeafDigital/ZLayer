//! Axum handlers for Raft RPC endpoints
//!
//! These handlers receive Raft RPCs over HTTP and forward them
//! to the local Raft node for processing.

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest, InstallSnapshotResponse,
    SnapshotResponse, VoteRequest, VoteResponse,
};
use openraft::{Vote, Snapshot};
use serde::Deserialize;
use tracing::{debug, error};

use crate::raft::{NodeId, RaftCoordinator, TypeConfig};

/// Handler for AppendEntries RPC
///
/// Receives log entries from the leader and appends them to the local log.
pub async fn handle_append_entries(
    State(raft): State<Arc<RaftCoordinator>>,
    Json(request): Json<AppendEntriesRequest<TypeConfig>>,
) -> Result<Json<AppendEntriesResponse<NodeId>>, AppError> {
    debug!(
        leader_id = ?request.vote.leader_id,
        term = request.vote.leader_id.term,
        prev_log_id = ?request.prev_log_id,
        entries_len = request.entries.len(),
        "Received append_entries RPC"
    );

    // Forward to the Raft node
    let raft_node = raft.raft_handle();
    let response = raft_node
        .append_entries(request)
        .await
        .map_err(|e| AppError::RaftError(format!("AppendEntries failed: {}", e)))?;

    Ok(Json(response))
}

/// Handler for InstallSnapshot RPC
///
/// Receives a snapshot from the leader to bring this node up to date.
pub async fn handle_install_snapshot(
    State(raft): State<Arc<RaftCoordinator>>,
    Json(request): Json<InstallSnapshotRequest<TypeConfig>>,
) -> Result<Json<InstallSnapshotResponse<NodeId>>, AppError> {
    debug!(
        leader_id = ?request.vote.leader_id,
        term = request.vote.leader_id.term,
        last_log_id = ?request.meta.last_log_id,
        "Received install_snapshot RPC"
    );

    // Forward to the Raft node
    let raft_node = raft.raft_handle();
    let response = raft_node
        .install_snapshot(request)
        .await
        .map_err(|e| AppError::RaftError(format!("InstallSnapshot failed: {}", e)))?;

    Ok(Json(response))
}

/// Handler for Vote RPC
///
/// Receives a vote request from a candidate during leader election.
pub async fn handle_vote(
    State(raft): State<Arc<RaftCoordinator>>,
    Json(request): Json<VoteRequest<NodeId>>,
) -> Result<Json<VoteResponse<NodeId>>, AppError> {
    debug!(
        candidate_id = ?request.vote.leader_id,
        term = request.vote.leader_id.term,
        last_log_id = ?request.last_log_id,
        "Received vote RPC"
    );

    // Forward to the Raft node
    let raft_node = raft.raft_handle();
    let response = raft_node
        .vote(request)
        .await
        .map_err(|e| AppError::RaftError(format!("Vote failed: {}", e)))?;

    Ok(Json(response))
}

/// Request structure for full snapshot RPC
#[derive(Deserialize)]
pub struct FullSnapshotRequest {
    pub vote: Vote<NodeId>,
    pub snapshot_data: Vec<u8>,
}

/// Handler for full snapshot RPC
///
/// Receives a full snapshot from the leader for replication.
pub async fn handle_full_snapshot(
    State(raft): State<Arc<RaftCoordinator>>,
    Json(request): Json<FullSnapshotRequest>,
) -> Result<Json<SnapshotResponse<NodeId>>, AppError> {
    debug!(
        leader_id = ?request.vote.leader_id,
        term = request.vote.leader_id.term,
        snapshot_size = request.snapshot_data.len(),
        "Received full_snapshot RPC"
    );

    // Create a snapshot from the received data
    let _snapshot: Snapshot<TypeConfig> = Snapshot {
        meta: openraft::SnapshotMeta {
            last_log_id: None, // Will be determined by the snapshot data
            last_membership: openraft::StoredMembership::default(),
            snapshot_id: format!("snapshot-{}", request.vote.leader_id.term),
        },
        snapshot: Box::new(std::io::Cursor::new(request.snapshot_data)),
    };

    // Forward to the Raft node
    let _raft_node = raft.raft_handle();

    // Note: OpenRaft v0.9 doesn't have a direct full_snapshot method on the Raft handle.
    // This is typically handled through the streaming API. For now, return an error
    // indicating this needs proper implementation.
    Err(AppError::RaftError(
        "Full snapshot streaming not yet implemented".to_string(),
    ))
}

/// Application error type for HTTP handlers
#[derive(Debug)]
pub enum AppError {
    RaftError(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AppError::RaftError(msg) => {
                error!(error = %msg, "Raft RPC handler error");
                (StatusCode::INTERNAL_SERVER_ERROR, msg)
            }
        };

        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}
