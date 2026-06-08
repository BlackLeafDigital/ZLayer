//! Edge-cache eligibility API DTOs.
//!
//! Wire types for the edge-cache eligibility endpoints exposed by
//! `zlayer-api` and consumed by upstream control-plane callers (the
//! consuming cluster manager). The host-side cache fill,
//! eviction, hit-counting subsystem is out of scope here — these types
//! describe only the API surface used to mark a node eligible or
//! ineligible for edge caching and to read placeholder hit/miss counters.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Per-node capacity advertised when registering for edge-cache eligibility.
///
/// Mirrors `zlayer_overlay::edge_cache::NodeCapacity` 1:1 over the wire.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct NodeCapacityDto {
    /// Number of CPU cores the node is willing to dedicate to cache work.
    pub cpu_cores: u32,
    /// Resident memory (MiB) the node will allow the cache to occupy.
    pub ram_mib: u64,
    /// On-disk cache budget (MiB).
    pub disk_mib: u64,
    /// Optional geo / region label (e.g. `"us-east-1"`) used by future
    /// placement decisions. `None` means "no geo preference declared".
    #[serde(default)]
    pub geo: Option<String>,
}

/// Body for `POST /api/v1/nodes/{node_id}/edge-cache`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct EnableEdgeCacheRequest {
    /// Capacity this node is willing to contribute to the edge cache.
    pub capacity: NodeCapacityDto,
}

/// Response body for `GET /api/v1/nodes/{node_id}/edge-cache/stats`.
///
/// Stats are placeholders that always report zeroes today — the real
/// hit/miss counters land in the follow-on cache subsystem. The endpoint
/// exists now so upstream integration tests (Zatabase Wave 1.3.3.7) can
/// build against a stable shape.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct EdgeCacheStatsResponse {
    /// Total cache hits observed for this node since registration.
    pub hits: u64,
    /// Total cache misses observed for this node since registration.
    pub misses: u64,
}
