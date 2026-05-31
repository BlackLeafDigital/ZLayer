//! Edge-cache eligibility endpoints.
//!
//! Thin HTTP wrapper around [`zlayer_overlay::edge_cache::EdgeCacheRegistry`].
//! Three endpoints:
//!
//! - `POST   /api/v1/nodes/{node_id}/edge-cache`        — enable
//! - `DELETE /api/v1/nodes/{node_id}/edge-cache`        — disable
//! - `GET    /api/v1/nodes/{node_id}/edge-cache/stats`  — placeholder stats
//!
//! Designed for upstream control-plane callers (notably Zatabase's
//! `zql_cluster::ClusterManager`) that need to mark `ZLayer` nodes
//! eligible for edge caching. The actual cache fill/eviction subsystem is
//! out of scope here — these endpoints only twiddle eligibility state and
//! its gossip-label broadcast.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use zlayer_overlay::edge_cache::{EdgeCacheError, EdgeCacheRegistry, NodeCapacity};

pub use zlayer_types::api::edge_cache::*;

use crate::auth::AuthUser;
use crate::error::{ApiError, Result};

// =============================================================================
// State
// =============================================================================

/// Axum-router state holding the shared [`EdgeCacheRegistry`].
///
/// `Clone` is cheap (the underlying registry is `Arc`-wrapped). Built by
/// the daemon during startup via [`EdgeCacheApiState::with_registry`] so
/// the API handlers and any in-process subsystem share the same registry.
#[derive(Clone)]
pub struct EdgeCacheApiState {
    registry: Arc<EdgeCacheRegistry>,
}

impl EdgeCacheApiState {
    /// Build a state backed by a fresh in-memory registry with no gossip
    /// pool attached. Used by tests and standalone-mode daemons.
    #[must_use]
    pub fn new() -> Self {
        Self {
            registry: Arc::new(EdgeCacheRegistry::new()),
        }
    }

    /// Build a state that shares the supplied [`EdgeCacheRegistry`] with
    /// other in-process subsystems (e.g. a future cache filler that
    /// consumes the same eligibility data via [`EdgeCacheRegistry::list_eligible`]).
    #[must_use]
    pub fn with_registry(registry: Arc<EdgeCacheRegistry>) -> Self {
        Self { registry }
    }

    /// Borrow the shared registry (used by tests + future in-process callers).
    #[must_use]
    pub fn registry(&self) -> Arc<EdgeCacheRegistry> {
        Arc::clone(&self.registry)
    }
}

impl Default for EdgeCacheApiState {
    fn default() -> Self {
        Self::new()
    }
}

impl From<EdgeCacheError> for ApiError {
    fn from(err: EdgeCacheError) -> Self {
        match err {
            EdgeCacheError::NodeNotFound(_) => ApiError::NotFound(err.to_string()),
            EdgeCacheError::Gossip(_) => ApiError::Internal(err.to_string()),
        }
    }
}

// =============================================================================
// Handlers
// =============================================================================

/// Mark a node as eligible for edge caching.
///
/// # Errors
///
/// Returns `500 Internal Server Error` if the gossip self-info
/// re-announcement fails (rare — chitchat only rejects on serialization
/// errors).
#[utoipa::path(
    post,
    path = "/api/v1/nodes/{node_id}/edge-cache",
    params(
        ("node_id" = u64, Path, description = "Target ZLayer node ID"),
    ),
    request_body = EnableEdgeCacheRequest,
    responses(
        (status = 204, description = "Node marked edge-cache-eligible"),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal error"),
    ),
    security(("bearer_auth" = [])),
    tag = "EdgeCache"
)]
pub async fn enable_edge_cache(
    _user: AuthUser,
    Path(node_id): Path<u64>,
    State(state): State<EdgeCacheApiState>,
    Json(request): Json<EnableEdgeCacheRequest>,
) -> Result<StatusCode> {
    let capacity = NodeCapacity {
        cpu_cores: request.capacity.cpu_cores,
        ram_mib: request.capacity.ram_mib,
        disk_mib: request.capacity.disk_mib,
        geo: request.capacity.geo,
    };
    state.registry.enable(node_id, capacity).await?;
    tracing::info!(node_id, "edge-cache eligibility enabled");
    Ok(StatusCode::NO_CONTENT)
}

/// Remove a node from the edge-cache eligibility registry.
///
/// # Errors
///
/// Returns `404 Not Found` if the node was not currently registered.
#[utoipa::path(
    delete,
    path = "/api/v1/nodes/{node_id}/edge-cache",
    params(
        ("node_id" = u64, Path, description = "Target ZLayer node ID"),
    ),
    responses(
        (status = 204, description = "Node removed from edge-cache eligibility"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Node was not registered"),
    ),
    security(("bearer_auth" = [])),
    tag = "EdgeCache"
)]
pub async fn disable_edge_cache(
    _user: AuthUser,
    Path(node_id): Path<u64>,
    State(state): State<EdgeCacheApiState>,
) -> Result<StatusCode> {
    state.registry.disable(node_id).await?;
    tracing::info!(node_id, "edge-cache eligibility disabled");
    Ok(StatusCode::NO_CONTENT)
}

/// Return placeholder cache hit/miss counters for a node.
///
/// Stats are a documented stub: today this always returns `{hits: 0,
/// misses: 0}` regardless of `node_id`. The endpoint exists so upstream
/// integration tests (Zatabase Wave 1.3.3.7) can build against the
/// stable shape; real counters land in the follow-on cache subsystem.
#[utoipa::path(
    get,
    path = "/api/v1/nodes/{node_id}/edge-cache/stats",
    params(
        ("node_id" = u64, Path, description = "Target ZLayer node ID"),
    ),
    responses(
        (status = 200, description = "Placeholder hit/miss counters", body = EdgeCacheStatsResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "EdgeCache"
)]
pub async fn edge_cache_stats(
    _user: AuthUser,
    Path(node_id): Path<u64>,
    State(state): State<EdgeCacheApiState>,
) -> Json<EdgeCacheStatsResponse> {
    let s = state.registry.stats(node_id).await;
    Json(EdgeCacheStatsResponse {
        hits: s.hits,
        misses: s.misses,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn state_round_trip_via_registry() {
        let state = EdgeCacheApiState::new();
        let reg = state.registry();
        reg.enable(
            42,
            NodeCapacity {
                cpu_cores: 2,
                ram_mib: 256,
                disk_mib: 1024,
                geo: None,
            },
        )
        .await
        .expect("enable");
        assert!(reg.is_enabled(42).await);
        reg.disable(42).await.expect("disable");
        assert!(!reg.is_enabled(42).await);
    }
}
