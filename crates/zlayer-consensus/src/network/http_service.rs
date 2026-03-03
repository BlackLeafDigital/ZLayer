//! Axum HTTP service for receiving Raft RPCs.
//!
//! Provides an Axum router with endpoints for all Raft RPC operations.
//! Uses **bincode** serialization for request/response bodies.
//!
//! ## Endpoints
//!
//! | Method | Path | Description |
//! |--------|------|-------------|
//! | POST | `/raft/vote` | RequestVote RPC |
//! | POST | `/raft/append` | AppendEntries RPC |
//! | POST | `/raft/snapshot` | InstallSnapshot RPC |
//! | POST | `/raft/full-snapshot` | Full snapshot transfer |

use std::io::Cursor;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;
use openraft::raft::{AppendEntriesRequest, InstallSnapshotRequest, VoteRequest};
use openraft::storage::Snapshot;
use openraft::{BasicNode, Raft, RaftTypeConfig, SnapshotMeta, Vote};
use tracing::{debug, error};

use crate::types::NodeId;

/// Application state shared with Axum handlers.
struct RaftState<C: RaftTypeConfig> {
    raft: Raft<C>,
}

/// Create an Axum router for Raft RPC endpoints.
///
/// The router uses bincode for serialization. Mount it at any prefix:
///
/// ```ignore
/// let raft_router = raft_service_router(raft_instance);
/// let app = Router::new().nest("/", raft_router);
/// ```
pub fn raft_service_router<C>(raft: Raft<C>) -> Router
where
    C: RaftTypeConfig<NodeId = NodeId, Node = BasicNode, SnapshotData = Cursor<Vec<u8>>>,
    C::D: serde::Serialize + serde::de::DeserializeOwned,
    C::R: serde::Serialize + serde::de::DeserializeOwned,
    C::Entry: serde::Serialize + serde::de::DeserializeOwned,
{
    let state = Arc::new(RaftState { raft });

    Router::new()
        .route("/raft/vote", post(handle_vote::<C>))
        .route("/raft/append", post(handle_append::<C>))
        .route("/raft/snapshot", post(handle_snapshot::<C>))
        .route("/raft/full-snapshot", post(handle_full_snapshot::<C>))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn handle_vote<C>(
    State(state): State<Arc<RaftState<C>>>,
    body: Bytes,
) -> impl IntoResponse
where
    C: RaftTypeConfig<NodeId = NodeId, Node = BasicNode, SnapshotData = Cursor<Vec<u8>>>,
    C::D: serde::Serialize + serde::de::DeserializeOwned,
    C::R: serde::Serialize + serde::de::DeserializeOwned,
    C::Entry: serde::Serialize + serde::de::DeserializeOwned,
{
    let req: VoteRequest<NodeId> = match bincode::deserialize(&body) {
        Ok(r) => r,
        Err(e) => {
            error!("Failed to deserialize vote request: {e}");
            return (StatusCode::BAD_REQUEST, Vec::new());
        }
    };

    debug!("Received vote RPC");

    match state.raft.vote(req).await {
        Ok(resp) => match bincode::serialize(&resp) {
            Ok(bytes) => (StatusCode::OK, bytes),
            Err(e) => {
                error!("Failed to serialize vote response: {e}");
                (StatusCode::INTERNAL_SERVER_ERROR, Vec::new())
            }
        },
        Err(e) => {
            error!("Vote RPC failed: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Vec::new())
        }
    }
}

async fn handle_append<C>(
    State(state): State<Arc<RaftState<C>>>,
    body: Bytes,
) -> impl IntoResponse
where
    C: RaftTypeConfig<NodeId = NodeId, Node = BasicNode, SnapshotData = Cursor<Vec<u8>>>,
    C::D: serde::Serialize + serde::de::DeserializeOwned,
    C::R: serde::Serialize + serde::de::DeserializeOwned,
    C::Entry: serde::Serialize + serde::de::DeserializeOwned,
{
    let req: AppendEntriesRequest<C> = match bincode::deserialize(&body) {
        Ok(r) => r,
        Err(e) => {
            error!("Failed to deserialize append request: {e}");
            return (StatusCode::BAD_REQUEST, Vec::new());
        }
    };

    debug!("Received append_entries RPC");

    match state.raft.append_entries(req).await {
        Ok(resp) => match bincode::serialize(&resp) {
            Ok(bytes) => (StatusCode::OK, bytes),
            Err(e) => {
                error!("Failed to serialize append response: {e}");
                (StatusCode::INTERNAL_SERVER_ERROR, Vec::new())
            }
        },
        Err(e) => {
            error!("AppendEntries RPC failed: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Vec::new())
        }
    }
}

async fn handle_snapshot<C>(
    State(state): State<Arc<RaftState<C>>>,
    body: Bytes,
) -> impl IntoResponse
where
    C: RaftTypeConfig<NodeId = NodeId, Node = BasicNode, SnapshotData = Cursor<Vec<u8>>>,
    C::D: serde::Serialize + serde::de::DeserializeOwned,
    C::R: serde::Serialize + serde::de::DeserializeOwned,
    C::Entry: serde::Serialize + serde::de::DeserializeOwned,
{
    let req: InstallSnapshotRequest<C> = match bincode::deserialize(&body) {
        Ok(r) => r,
        Err(e) => {
            error!("Failed to deserialize snapshot request: {e}");
            return (StatusCode::BAD_REQUEST, Vec::new());
        }
    };

    debug!("Received install_snapshot RPC");

    match state.raft.install_snapshot(req).await {
        Ok(resp) => match bincode::serialize(&resp) {
            Ok(bytes) => (StatusCode::OK, bytes),
            Err(e) => {
                error!("Failed to serialize snapshot response: {e}");
                (StatusCode::INTERNAL_SERVER_ERROR, Vec::new())
            }
        },
        Err(e) => {
            error!("InstallSnapshot RPC failed: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Vec::new())
        }
    }
}

#[derive(serde::Deserialize)]
struct FullSnapshotReq {
    vote: Vote<NodeId>,
    meta: SnapshotMeta<NodeId, BasicNode>,
    snapshot_data: Vec<u8>,
}

async fn handle_full_snapshot<C>(
    State(state): State<Arc<RaftState<C>>>,
    body: Bytes,
) -> impl IntoResponse
where
    C: RaftTypeConfig<NodeId = NodeId, Node = BasicNode, SnapshotData = Cursor<Vec<u8>>>,
    C::D: serde::Serialize + serde::de::DeserializeOwned,
    C::R: serde::Serialize + serde::de::DeserializeOwned,
    C::Entry: serde::Serialize + serde::de::DeserializeOwned,
{
    let req: FullSnapshotReq = match bincode::deserialize(&body) {
        Ok(r) => r,
        Err(e) => {
            error!("Failed to deserialize full snapshot request: {e}");
            return (StatusCode::BAD_REQUEST, Vec::new());
        }
    };

    debug!("Received full_snapshot RPC");

    let snapshot = Snapshot {
        meta: req.meta,
        snapshot: Box::new(Cursor::new(req.snapshot_data)),
    };

    match state.raft.install_full_snapshot(req.vote, snapshot).await {
        Ok(resp) => match bincode::serialize(&resp) {
            Ok(bytes) => (StatusCode::OK, bytes),
            Err(e) => {
                error!("Failed to serialize full snapshot response: {e}");
                (StatusCode::INTERNAL_SERVER_ERROR, Vec::new())
            }
        },
        Err(e) => {
            error!("install_full_snapshot failed: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Vec::new())
        }
    }
}
