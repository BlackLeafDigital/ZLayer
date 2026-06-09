//! Edge-cache eligibility registry.
//!
//! Tracks which nodes have been registered as eligible for edge caching by
//! the upstream control plane (the consuming cluster manager).
//! Propagates the eligibility decision via
//! the gossip-label mechanism in [`crate::gossip`] so peer nodes can route
//! cache-bearing traffic to eligible nodes by inspecting their advertised
//! [`gossip::PeerInfo::labels`](crate::gossip::PeerInfo).
//!
//! The actual cache fill / eviction / hit-counting logic is out of scope
//! for this module — it lives in upstream feature work (`Z3Fungi` sidecar or
//! a future overlay-native cache primitive). [`EdgeCacheRegistry::stats`]
//! returns a `(0, 0)` placeholder intentionally; the API surface exists so
//! upstream integration tests (e.g. Zatabase Wave 1.3.3.7) can build
//! against it.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use thiserror::Error;
use tokio::sync::RwLock;

use crate::error::OverlayError;
use crate::gossip::{GossipPool, PeerInfo};

/// Cache capacity advertised by a node when it becomes eligible.
#[derive(Debug, Clone)]
pub struct NodeCapacity {
    /// Number of CPU cores the node is willing to dedicate to cache work.
    pub cpu_cores: u32,
    /// Resident memory (MiB) the node will allow the cache to occupy.
    pub ram_mib: u64,
    /// On-disk cache budget (MiB).
    pub disk_mib: u64,
    /// Optional geo / region label used by future placement decisions.
    pub geo: Option<String>,
}

/// One entry in the registry — a node currently marked edge-cache-eligible.
#[derive(Debug, Clone)]
pub struct EdgeCacheNode {
    /// `ZLayer` node ID this entry describes.
    pub node_id: u64,
    /// Capacity the node declared at registration time.
    pub capacity: NodeCapacity,
    /// Wall-clock time the node was added to the registry.
    pub enabled_at: SystemTime,
}

/// Per-node cache hit/miss counters.
///
/// This is a documented placeholder: today both fields are always `0`.
/// The real counters will land in a follow-on patch when the cache fill /
/// eviction subsystem itself is implemented. The type lives here now so
/// upstream callers (Zatabase Wave 1.3.3.7 in particular) can build
/// integration tests against the stable shape.
#[derive(Debug, Clone, Copy, Default)]
pub struct EdgeCacheStats {
    /// Total cache hits observed for the node since registration.
    pub hits: u64,
    /// Total cache misses observed for the node since registration.
    pub misses: u64,
}

/// Errors returned by [`EdgeCacheRegistry`].
#[derive(Debug, Error)]
pub enum EdgeCacheError {
    /// Re-publishing the gossip self-info (carrying the updated labels)
    /// failed at the underlying chitchat layer.
    #[error("gossip label push failed: {0}")]
    Gossip(String),
    /// Caller asked to disable a node that wasn't registered. Treated as
    /// an error so the caller knows their view of the world is stale.
    #[error("node {0} is not registered as edge-cache eligible")]
    NodeNotFound(u64),
}

impl From<OverlayError> for EdgeCacheError {
    fn from(err: OverlayError) -> Self {
        EdgeCacheError::Gossip(err.to_string())
    }
}

/// Standard gossip-label keys this registry writes/clears.
const LABEL_KEY_ENABLED: &str = "edge_cache";
const LABEL_KEY_CPU: &str = "edge_cache_cpu";
const LABEL_KEY_RAM_MIB: &str = "edge_cache_ram_mib";
const LABEL_KEY_DISK_MIB: &str = "edge_cache_disk_mib";
const LABEL_KEY_GEO: &str = "edge_cache_geo";

/// Tracks which nodes are eligible for edge caching and propagates that
/// state via gossip labels.
///
/// Construction is cheap; the registry is `Clone` (it's an
/// `Arc`-wrapped pair of state + gossip handle) so it can be plumbed into
/// Axum handlers behind `with_state`.
#[derive(Clone)]
pub struct EdgeCacheRegistry {
    inner: Arc<RwLock<HashMap<u64, EdgeCacheNode>>>,
    /// Gossip pool used to broadcast the `edge_cache=true` label, plus
    /// the capacity sub-labels. `None` in test/standalone modes where no
    /// gossip pool is wired — the registry still tracks eligibility
    /// locally but doesn't propagate.
    gossip: Option<Arc<GossipPool>>,
    /// Self-info template used when re-announcing after a label change.
    /// `None` when [`gossip`] is also `None`.
    self_info: Arc<RwLock<Option<PeerInfo>>>,
}

impl std::fmt::Debug for EdgeCacheRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EdgeCacheRegistry")
            .field("gossip_attached", &self.gossip.is_some())
            .finish_non_exhaustive()
    }
}

impl EdgeCacheRegistry {
    /// Create a registry with no gossip pool attached.
    ///
    /// Eligibility decisions are still tracked locally and returned by
    /// [`Self::list_eligible`], but no labels are pushed to peers. Used
    /// by tests and by standalone-mode daemons that don't participate in
    /// the worker-tier gossip pool.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            gossip: None,
            self_info: Arc::new(RwLock::new(None)),
        }
    }

    /// Create a registry that pushes eligibility labels into the
    /// supplied gossip pool.
    ///
    /// `self_info` is the [`PeerInfo`] template the registry will mutate
    /// (label keys are added on `enable` and removed on `disable`) and
    /// then re-broadcast via [`GossipPool::announce_self`].
    #[must_use]
    pub fn with_gossip(gossip: Arc<GossipPool>, self_info: PeerInfo) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            gossip: Some(gossip),
            self_info: Arc::new(RwLock::new(Some(self_info))),
        }
    }

    /// Mark `node_id` as eligible for edge caching with `capacity`.
    ///
    /// Inserts a new [`EdgeCacheNode`] into the registry (overwriting any
    /// prior entry for the same `node_id`) and, when a gossip pool is
    /// attached, pushes the standard label set into the local node's
    /// gossip self-info and re-announces it.
    ///
    /// # Errors
    ///
    /// Returns [`EdgeCacheError::Gossip`] if the gossip pool rejects the
    /// re-announcement (serialization error inside chitchat).
    pub async fn enable(&self, node_id: u64, capacity: NodeCapacity) -> Result<(), EdgeCacheError> {
        {
            let mut guard = self.inner.write().await;
            guard.insert(
                node_id,
                EdgeCacheNode {
                    node_id,
                    capacity: capacity.clone(),
                    enabled_at: SystemTime::now(),
                },
            );
        }

        if let Some(gossip) = self.gossip.as_ref() {
            let mut info_guard = self.self_info.write().await;
            if let Some(info) = info_guard.as_mut() {
                info.labels
                    .insert(LABEL_KEY_ENABLED.to_string(), "true".to_string());
                info.labels
                    .insert(LABEL_KEY_CPU.to_string(), capacity.cpu_cores.to_string());
                info.labels
                    .insert(LABEL_KEY_RAM_MIB.to_string(), capacity.ram_mib.to_string());
                info.labels.insert(
                    LABEL_KEY_DISK_MIB.to_string(),
                    capacity.disk_mib.to_string(),
                );
                if let Some(geo) = capacity.geo.as_ref() {
                    info.labels.insert(LABEL_KEY_GEO.to_string(), geo.clone());
                } else {
                    info.labels.remove(LABEL_KEY_GEO);
                }
                gossip.announce_self(info).await?;
            }
        }

        Ok(())
    }

    /// Remove `node_id` from the eligibility registry.
    ///
    /// When a gossip pool is attached, clears the edge-cache label set
    /// from the local node's gossip self-info and re-announces.
    ///
    /// # Errors
    ///
    /// - [`EdgeCacheError::NodeNotFound`] if `node_id` was not registered.
    /// - [`EdgeCacheError::Gossip`] if the gossip re-announcement fails.
    pub async fn disable(&self, node_id: u64) -> Result<(), EdgeCacheError> {
        {
            let mut guard = self.inner.write().await;
            if guard.remove(&node_id).is_none() {
                return Err(EdgeCacheError::NodeNotFound(node_id));
            }
        }

        if let Some(gossip) = self.gossip.as_ref() {
            let mut info_guard = self.self_info.write().await;
            if let Some(info) = info_guard.as_mut() {
                info.labels.remove(LABEL_KEY_ENABLED);
                info.labels.remove(LABEL_KEY_CPU);
                info.labels.remove(LABEL_KEY_RAM_MIB);
                info.labels.remove(LABEL_KEY_DISK_MIB);
                info.labels.remove(LABEL_KEY_GEO);
                gossip.announce_self(info).await?;
            }
        }

        Ok(())
    }

    /// Snapshot of every currently-eligible node.
    pub async fn list_eligible(&self) -> Vec<EdgeCacheNode> {
        let guard = self.inner.read().await;
        guard.values().cloned().collect()
    }

    /// Whether `node_id` is currently registered as edge-cache-eligible.
    pub async fn is_enabled(&self, node_id: u64) -> bool {
        let guard = self.inner.read().await;
        guard.contains_key(&node_id)
    }

    /// Return placeholder hit/miss counters for `node_id`.
    ///
    /// **Documented stub**: always returns `(0, 0)` regardless of
    /// `node_id`. The real counters land in the follow-on cache subsystem
    /// (see module docs). This stays callable so upstream integration
    /// tests can rely on the endpoint existing today. The `async` shape is
    /// preserved so a future implementation can read shared cache state
    /// without a breaking API churn.
    #[allow(clippy::unused_async)]
    pub async fn stats(&self, _node_id: u64) -> EdgeCacheStats {
        EdgeCacheStats::default()
    }
}

impl Default for EdgeCacheRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(cpu: u32, ram: u64, disk: u64, geo: Option<&str>) -> NodeCapacity {
        NodeCapacity {
            cpu_cores: cpu,
            ram_mib: ram,
            disk_mib: disk,
            geo: geo.map(str::to_string),
        }
    }

    #[tokio::test]
    async fn enable_then_disable_round_trip() {
        let reg = EdgeCacheRegistry::new();
        assert!(reg.list_eligible().await.is_empty());
        assert!(!reg.is_enabled(7).await);

        reg.enable(7, cap(4, 1024, 8192, Some("us-east-1")))
            .await
            .expect("enable");
        assert!(reg.is_enabled(7).await);
        let listed = reg.list_eligible().await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].node_id, 7);
        assert_eq!(listed[0].capacity.cpu_cores, 4);
        assert_eq!(listed[0].capacity.geo.as_deref(), Some("us-east-1"));

        reg.disable(7).await.expect("disable");
        assert!(!reg.is_enabled(7).await);
        assert!(reg.list_eligible().await.is_empty());
    }

    #[tokio::test]
    async fn disable_unknown_node_errors() {
        let reg = EdgeCacheRegistry::new();
        let err = reg.disable(99).await.expect_err("should fail");
        match err {
            EdgeCacheError::NodeNotFound(99) => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn stats_returns_placeholder_zero() {
        let reg = EdgeCacheRegistry::new();
        reg.enable(3, cap(1, 64, 256, None)).await.expect("enable");
        let s = reg.stats(3).await;
        assert_eq!(s.hits, 0);
        assert_eq!(s.misses, 0);
        // Unknown node also returns zeroes — stats is purely a stub.
        let s2 = reg.stats(999).await;
        assert_eq!(s2.hits, 0);
        assert_eq!(s2.misses, 0);
    }

    #[tokio::test]
    async fn enable_overwrites_capacity() {
        let reg = EdgeCacheRegistry::new();
        reg.enable(5, cap(2, 128, 512, None)).await.expect("enable");
        reg.enable(5, cap(8, 4096, 16_384, Some("eu-west-1")))
            .await
            .expect("re-enable");
        let listed = reg.list_eligible().await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].capacity.cpu_cores, 8);
        assert_eq!(listed[0].capacity.disk_mib, 16_384);
        assert_eq!(listed[0].capacity.geo.as_deref(), Some("eu-west-1"));
    }
}
