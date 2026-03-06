//! Axum HTTP service for receiving Raft RPCs.
//!
//! Provides an Axum router with endpoints for all Raft RPC operations.
//! Uses **postcard2** serialization for request/response bodies.
//!
//! When an `auth_token` is provided, every request must include an
//! `Authorization: Bearer <token>` header matching the expected value.
//! Requests without a valid token receive HTTP 401.
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
use axum::http::{Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use openraft::raft::{AppendEntriesRequest, InstallSnapshotRequest, VoteRequest};
use openraft::storage::Snapshot;
use openraft::{BasicNode, Raft, RaftTypeConfig, SnapshotMeta, Vote};
use tracing::{debug, error, warn};

use crate::types::NodeId;

/// Application state shared with Axum handlers.
struct RaftState<C: RaftTypeConfig> {
    raft: Raft<C>,
}

/// Create an Axum router for Raft RPC endpoints.
///
/// If `auth_token` is `Some`, a middleware layer is added that validates
/// the `Authorization: Bearer <token>` header on every request.  Requests
/// that do not carry a matching token are rejected with HTTP 401.
///
/// The router uses postcard2 for serialization. Mount it at any prefix:
///
/// ```ignore
/// let raft_router = raft_service_router(raft_instance, Some("secret".into()));
/// let app = Router::new().nest("/", raft_router);
/// ```
pub fn raft_service_router<C>(raft: Raft<C>, auth_token: Option<String>) -> Router
where
    C: RaftTypeConfig<NodeId = NodeId, Node = BasicNode, SnapshotData = Cursor<Vec<u8>>>,
    C::D: serde::Serialize + serde::de::DeserializeOwned,
    C::R: serde::Serialize + serde::de::DeserializeOwned,
    C::Entry: serde::Serialize + serde::de::DeserializeOwned,
{
    let state = Arc::new(RaftState { raft });

    let router = Router::new()
        .route("/raft/vote", post(handle_vote::<C>))
        .route("/raft/append", post(handle_append::<C>))
        .route("/raft/snapshot", post(handle_snapshot::<C>))
        .route("/raft/full-snapshot", post(handle_full_snapshot::<C>))
        .with_state(state);

    if let Some(token) = auth_token {
        let expected = Arc::new(token);
        router.layer(middleware::from_fn(move |req, next| {
            let expected = Arc::clone(&expected);
            bearer_auth_middleware(expected, req, next)
        }))
    } else {
        router
    }
}

/// Middleware that validates `Authorization: Bearer <token>`.
async fn bearer_auth_middleware(
    expected_token: Arc<String>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let auth_header = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    match auth_header {
        Some(value) if value.starts_with("Bearer ") => {
            let provided = &value["Bearer ".len()..];
            if provided == expected_token.as_str() {
                next.run(req).await
            } else {
                warn!("Raft RPC rejected: invalid bearer token");
                StatusCode::UNAUTHORIZED.into_response()
            }
        }
        _ => {
            warn!("Raft RPC rejected: missing or malformed Authorization header");
            StatusCode::UNAUTHORIZED.into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn handle_vote<C>(State(state): State<Arc<RaftState<C>>>, body: Bytes) -> impl IntoResponse
where
    C: RaftTypeConfig<NodeId = NodeId, Node = BasicNode, SnapshotData = Cursor<Vec<u8>>>,
    C::D: serde::Serialize + serde::de::DeserializeOwned,
    C::R: serde::Serialize + serde::de::DeserializeOwned,
    C::Entry: serde::Serialize + serde::de::DeserializeOwned,
{
    let req: VoteRequest<NodeId> = match postcard2::from_bytes(&body) {
        Ok(r) => r,
        Err(e) => {
            error!("Failed to deserialize vote request: {e}");
            return (StatusCode::BAD_REQUEST, Vec::new());
        }
    };

    debug!("Received vote RPC");

    match state.raft.vote(req).await {
        Ok(resp) => match postcard2::to_vec(&resp) {
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

async fn handle_append<C>(State(state): State<Arc<RaftState<C>>>, body: Bytes) -> impl IntoResponse
where
    C: RaftTypeConfig<NodeId = NodeId, Node = BasicNode, SnapshotData = Cursor<Vec<u8>>>,
    C::D: serde::Serialize + serde::de::DeserializeOwned,
    C::R: serde::Serialize + serde::de::DeserializeOwned,
    C::Entry: serde::Serialize + serde::de::DeserializeOwned,
{
    let req: AppendEntriesRequest<C> = match postcard2::from_bytes(&body) {
        Ok(r) => r,
        Err(e) => {
            error!("Failed to deserialize append request: {e}");
            return (StatusCode::BAD_REQUEST, Vec::new());
        }
    };

    debug!("Received append_entries RPC");

    match state.raft.append_entries(req).await {
        Ok(resp) => match postcard2::to_vec(&resp) {
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
    let req: InstallSnapshotRequest<C> = match postcard2::from_bytes(&body) {
        Ok(r) => r,
        Err(e) => {
            error!("Failed to deserialize snapshot request: {e}");
            return (StatusCode::BAD_REQUEST, Vec::new());
        }
    };

    debug!("Received install_snapshot RPC");

    match state.raft.install_snapshot(req).await {
        Ok(resp) => match postcard2::to_vec(&resp) {
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
    let req: FullSnapshotReq = match postcard2::from_bytes(&body) {
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
        Ok(resp) => match postcard2::to_vec(&resp) {
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
