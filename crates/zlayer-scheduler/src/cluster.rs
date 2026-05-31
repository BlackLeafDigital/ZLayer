//! Cluster membership + dispatch abstraction.
//!
//! Three implementations:
//! - [`SingleNodeCluster`]: degenerate (no peers; everything is local).
//! - [`RaftCluster`]: openraft-backed; this node may be leader or follower.
//!   Implemented in this module but defined separately; see [`raft::RaftCluster`].
//! - [`StaticCluster`]: config-driven peer list; deterministic leader election.
//!
//! All three share a uniform [`Cluster`] trait so [`crate::Scheduler`] and
//! [`zlayer_agent::ServiceManager`] can be cluster-agnostic.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::raft::NodeId;

/// Local-dispatch callback used by every `Cluster` impl. Invoked when
/// `dispatch_scale` or `forward_scale` would target this node — instead
/// of a localhost round-trip, the cluster routes through this closure,
/// which the caller wires to `ServiceManager::scale_service_local`.
pub type LocalDispatch = Box<
    dyn Fn(
            InternalScaleRequest,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), ClusterError>> + Send>>
        + Send
        + Sync,
>;

/// Liveness state for a node in the cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeState {
    /// Ready to accept work.
    Ready,
    /// Heartbeat missed beyond the failure threshold. Existing containers
    /// continue running on the node, but no new work is dispatched here.
    Unreachable,
    /// Administratively quiesced. Existing containers continue running, new
    /// work is dispatched elsewhere, and the node will eventually exit.
    Draining,
}

/// A single node's record in the cluster directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRecord {
    /// Stable node identifier (matches openraft's `NodeId`).
    pub id: NodeId,
    /// Address of the node's HTTP API (used for cluster-internal RPCs).
    pub api_addr: SocketAddr,
    /// Free-form labels attached to the node (placement, `NodeSelector` matching).
    pub labels: HashMap<String, String>,
    /// Operating system this node runs (Linux / Windows / macOS).
    pub os: String,
    /// Current liveness state.
    pub state: NodeState,
    /// Last time we observed liveness for this node.
    pub last_seen: SystemTime,
}

/// Outcome of placing a single one-off container, returned by
/// [`Cluster::place_container`]. Carries enough for the caller (the
/// container-create handler in `zlayer-api`) to either create locally or
/// forward the request — without the scheduler crate needing to know the
/// `zlayer-api` request types.
#[derive(Debug, Clone)]
pub struct ContainerPlacement {
    /// The node chosen to host the container.
    pub node_id: NodeId,
    /// Base URL of the chosen node's API, e.g. `http://10.0.0.2:3669`.
    /// The caller appends the internal container-create path.
    pub api_base_url: String,
    /// True when the chosen node is the local node (create here, no forward).
    pub is_self: bool,
}

// Wire-level scale request types live in `zlayer-types::cluster` so they can
// be shared between the HTTP fan-out path (here) and the `/internal/scale`
// handler in `zlayer-api`. Re-exported under the original path so existing
// call sites (`zlayer_scheduler::cluster::InternalScaleRequest::new`,
// `dispatch_scale(&self, NodeId, InternalScaleRequest)`) keep compiling.
pub use zlayer_types::cluster::{InternalScaleRequest, ScaleAssignment};

/// Errors produced by [`Cluster`] operations.
#[derive(Debug, thiserror::Error)]
pub enum ClusterError {
    /// This node is not the current leader (raft mode) or not the
    /// elected dispatcher (static mode). Try forwarding to the leader.
    #[error("not the leader; current leader is {0:?}")]
    NotLeader(Option<NodeId>),

    /// No leader has been elected yet (raft pre-election, or all static
    /// peers are Unreachable).
    #[error("no leader available")]
    NoLeader,

    /// Target node not found in the cluster directory.
    #[error("target node {0} not found in cluster")]
    UnknownNode(NodeId),

    /// Target node is in a non-Ready state.
    #[error("target node {0} is not Ready (state: {1:?})")]
    NodeNotReady(NodeId, NodeState),

    /// HTTP dispatch returned a transport-level error.
    #[error("dispatch transport error: {0}")]
    Transport(String),

    /// HTTP dispatch returned a non-success status.
    #[error("dispatch HTTP {status}: {body}")]
    HttpStatus { status: u16, body: String },

    /// Underlying scheduler / raft error.
    #[error("scheduler: {0}")]
    Scheduler(#[from] Box<crate::error::SchedulerError>),
}

/// Cluster membership + dispatch abstraction.
///
/// Three implementations live in this crate (`SingleNodeCluster`,
/// `RaftCluster`, `StaticCluster`); a future `WorkerTierCluster` will fit
/// the same shape.
///
/// Methods are async because all three impls do I/O on at least some paths
/// (HTTP heartbeats, openraft state queries).
#[async_trait]
pub trait Cluster: Send + Sync {
    /// Stable identity of this node within the cluster.
    fn node_id(&self) -> NodeId;

    /// True if this node is currently authoritative for dispatch.
    ///
    /// - `SingleNodeCluster`: always `true`.
    /// - `RaftCluster`: true on the current raft leader.
    /// - `StaticCluster`: true on the deterministically-elected leader
    ///   (typically the lowest-id Ready peer).
    async fn is_leader(&self) -> bool;

    /// Address of the current leader, or `None` if no leader is currently
    /// known (e.g. raft hasn't elected yet, or all static peers are
    /// Unreachable).
    async fn leader_addr(&self) -> Option<SocketAddr>;

    /// All known nodes in the cluster, including this one. The Ready
    /// subset is the input domain for placement decisions.
    async fn list_nodes(&self) -> Vec<NodeRecord>;

    /// Dispatch a scale request to a specific node.
    ///
    /// Used by the leader to fan out per-node assignments. The target may
    /// be `self.node_id()`; implementations are expected to short-circuit
    /// to a local call in that case (no localhost HTTP round-trip).
    async fn dispatch_scale(
        &self,
        target: NodeId,
        req: InternalScaleRequest,
    ) -> Result<(), ClusterError>;

    /// Forward a scale request to the current leader.
    ///
    /// Called when a follower receives a deploy. Returns `NoLeader` if no
    /// leader is currently known.
    async fn forward_scale(&self, req: InternalScaleRequest) -> Result<(), ClusterError>;

    /// Place a single one-off container described by `spec`, honoring its
    /// `platform`, `node_selector`, and resource requests against the Ready
    /// node set. Returns the chosen node (with its API base URL) or `None`
    /// when no Ready node satisfies the constraints.
    ///
    /// Only meaningful on the leader / dispatch-authoritative node — callers
    /// should forward to the leader first when `is_leader()` is false.
    async fn place_container(
        &self,
        spec: &zlayer_spec::ServiceSpec,
    ) -> Result<Option<ContainerPlacement>, ClusterError>;
}

/// Build a single-node [`placement::NodeState`] from an OS string (the only
/// platform signal the `SingleNodeCluster` / `StaticCluster` carry). Arch and
/// resources are left unknown (wildcard) so a node with a recognized OS still
/// matches platform constraints on the OS axis without spurious arch/capacity
/// rejections.
fn node_state_from_os(
    id: NodeId,
    addr: SocketAddr,
    os: &str,
    labels: HashMap<String, String>,
) -> crate::placement::NodeState {
    crate::placement::NodeState {
        id,
        address: addr.to_string(),
        labels,
        resources: crate::placement::NodeResources::default(),
        healthy: true,
        os: zlayer_spec::OsKind::from_rust_os(os).or_else(|| zlayer_spec::OsKind::from_oci_str(os)),
        arch: None,
    }
}

// ============================================================================
// SingleNodeCluster
// ============================================================================

/// Degenerate `Cluster` impl for daemons with no peers.
///
/// `is_leader()` always returns true, `list_nodes()` returns just this node,
/// and dispatch routes everything to the local scale handler via a callback.
///
/// The callback is a `Box<dyn Fn(InternalScaleRequest) -> ...>` so we can
/// wire it to `ServiceManager::scale_service_local` without creating a
/// circular crate dep.
pub struct SingleNodeCluster {
    node_id: NodeId,
    api_addr: SocketAddr,
    os: String,
    local_dispatch: LocalDispatch,
}

impl SingleNodeCluster {
    /// Construct a single-node cluster bound to the given node identity and
    /// a local dispatch closure.
    #[must_use]
    pub fn new<F, Fut>(
        node_id: NodeId,
        api_addr: SocketAddr,
        os: impl Into<String>,
        local_dispatch: F,
    ) -> Self
    where
        F: Fn(InternalScaleRequest) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(), ClusterError>> + Send + 'static,
    {
        Self {
            node_id,
            api_addr,
            os: os.into(),
            local_dispatch: Box::new(move |req| Box::pin(local_dispatch(req))),
        }
    }
}

#[async_trait]
impl Cluster for SingleNodeCluster {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    async fn is_leader(&self) -> bool {
        true
    }

    async fn leader_addr(&self) -> Option<SocketAddr> {
        Some(self.api_addr)
    }

    async fn list_nodes(&self) -> Vec<NodeRecord> {
        vec![NodeRecord {
            id: self.node_id,
            api_addr: self.api_addr,
            labels: HashMap::new(),
            os: self.os.clone(),
            state: NodeState::Ready,
            last_seen: SystemTime::now(),
        }]
    }

    async fn dispatch_scale(
        &self,
        target: NodeId,
        req: InternalScaleRequest,
    ) -> Result<(), ClusterError> {
        if target != self.node_id {
            return Err(ClusterError::UnknownNode(target));
        }
        (self.local_dispatch)(req).await
    }

    async fn forward_scale(&self, req: InternalScaleRequest) -> Result<(), ClusterError> {
        // Single-node mode: we ARE the leader.
        (self.local_dispatch)(req).await
    }

    async fn place_container(
        &self,
        spec: &zlayer_spec::ServiceSpec,
    ) -> Result<Option<ContainerPlacement>, ClusterError> {
        let node = node_state_from_os(self.node_id, self.api_addr, &self.os, HashMap::new());
        Ok(
            crate::placement::place_single_container(spec, std::slice::from_ref(&node)).map(
                |node_id| ContainerPlacement {
                    node_id,
                    api_base_url: format!("http://{}", self.api_addr),
                    is_self: true,
                },
            ),
        )
    }
}

// ============================================================================
// RaftCluster
// ============================================================================

/// openraft-backed cluster. Leader election + node membership come from raft;
/// dispatch is HTTP to peers' `/api/v1/internal/scale` endpoints.
pub struct RaftCluster {
    /// This node's id. Stored explicitly to avoid an async hop on every
    /// `node_id()` call.
    node_id: NodeId,
    /// Raft handle (leadership, cluster state).
    raft: Arc<crate::raft::RaftCoordinator>,
    /// HTTP client for peer dispatch.
    http_client: reqwest::Client,
    /// Shared secret presented as `X-ZLayer-Internal-Token` on cluster RPCs.
    internal_token: String,
    /// Local dispatch closure (Phase 1 same as `SingleNodeCluster` — invoked
    /// when `dispatch_scale` targets this node, to avoid a localhost
    /// round-trip).
    local_dispatch: LocalDispatch,
}

impl RaftCluster {
    /// Build a `RaftCluster` wired into an existing `RaftCoordinator`.
    #[must_use]
    pub fn new<F, Fut>(
        node_id: NodeId,
        raft: Arc<crate::raft::RaftCoordinator>,
        http_client: reqwest::Client,
        internal_token: String,
        local_dispatch: F,
    ) -> Self
    where
        F: Fn(InternalScaleRequest) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(), ClusterError>> + Send + 'static,
    {
        Self {
            node_id,
            raft,
            http_client,
            internal_token,
            local_dispatch: Box::new(move |req| Box::pin(local_dispatch(req))),
        }
    }

    /// Resolve a node id to its `http://host:port` base URL, derived from raft
    /// state. Falls back to `127.0.0.1:3669` if the node has no
    /// `advertise_addr`.
    async fn resolve_url(&self, node_id: NodeId) -> Result<String, ClusterError> {
        let state = self.raft.read_state().await;
        let Some(node_info) = state.nodes.get(&node_id) else {
            return Err(ClusterError::UnknownNode(node_id));
        };
        let host = if node_info.advertise_addr.is_empty() {
            node_info
                .address
                .split(':')
                .next()
                .unwrap_or("127.0.0.1")
                .to_string()
        } else {
            node_info.advertise_addr.clone()
        };
        let port = if node_info.api_port > 0 {
            node_info.api_port
        } else {
            3669
        };
        Ok(format!("http://{host}:{port}/api/v1/internal/scale"))
    }

    async fn http_dispatch(
        &self,
        url: String,
        req: &InternalScaleRequest,
    ) -> Result<(), ClusterError> {
        let resp = self
            .http_client
            .post(&url)
            .header("X-ZLayer-Internal-Token", &self.internal_token)
            .json(req)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| ClusterError::Transport(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(ClusterError::HttpStatus { status, body });
        }
        Ok(())
    }

    /// Returns true if `node_id` is currently a known member of the raft
    /// cluster. Reads the raft state directly (cheap — no HTTP).
    pub async fn knows_node(&self, node_id: NodeId) -> bool {
        let state = self.raft.read_state().await;
        state.nodes.contains_key(&node_id)
    }
}

#[async_trait]
impl Cluster for RaftCluster {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    async fn is_leader(&self) -> bool {
        self.raft.leader_id() == Some(self.node_id)
    }

    async fn leader_addr(&self) -> Option<SocketAddr> {
        let leader_id = self.raft.leader_id()?;
        let state = self.raft.read_state().await;
        let node_info = state.nodes.get(&leader_id)?;
        let host = if node_info.advertise_addr.is_empty() {
            node_info.address.split(':').next().unwrap_or("127.0.0.1")
        } else {
            node_info.advertise_addr.as_str()
        };
        let port = if node_info.api_port > 0 {
            node_info.api_port
        } else {
            3669
        };
        format!("{host}:{port}").parse().ok()
    }

    async fn list_nodes(&self) -> Vec<NodeRecord> {
        let state = self.raft.read_state().await;
        state
            .nodes
            .iter()
            .map(|(id, info)| {
                let host = if info.advertise_addr.is_empty() {
                    info.address.split(':').next().unwrap_or("127.0.0.1")
                } else {
                    info.advertise_addr.as_str()
                };
                let port = if info.api_port > 0 {
                    info.api_port
                } else {
                    3669
                };
                let api_addr: SocketAddr = format!("{host}:{port}")
                    .parse()
                    .unwrap_or_else(|_| "127.0.0.1:3669".parse().unwrap());
                NodeRecord {
                    id: *id,
                    api_addr,
                    labels: HashMap::new(),
                    os: "linux".to_string(), // populated by metrics layer in a later phase
                    state: NodeState::Ready,
                    last_seen: SystemTime::now(),
                }
            })
            .collect()
    }

    async fn dispatch_scale(
        &self,
        target: NodeId,
        req: InternalScaleRequest,
    ) -> Result<(), ClusterError> {
        if target == self.node_id {
            // Short-circuit: avoid a localhost HTTP round-trip.
            return (self.local_dispatch)(req).await;
        }
        let url = self.resolve_url(target).await?;
        self.http_dispatch(url, &req).await
    }

    async fn forward_scale(&self, req: InternalScaleRequest) -> Result<(), ClusterError> {
        let leader_id = self.raft.leader_id().ok_or(ClusterError::NoLeader)?;
        if leader_id == self.node_id {
            // We ARE the leader — fall back to local dispatch.
            return (self.local_dispatch)(req).await;
        }
        let url = self.resolve_url(leader_id).await?;
        self.http_dispatch(url, &req).await
    }

    async fn place_container(
        &self,
        spec: &zlayer_spec::ServiceSpec,
    ) -> Result<Option<ContainerPlacement>, ClusterError> {
        let state = self.raft.read_state().await;
        let nodes = crate::cluster_nodes_to_node_states(&state.nodes);
        let Some(target) = crate::placement::place_single_container(spec, &nodes) else {
            return Ok(None);
        };
        let info = state
            .nodes
            .get(&target)
            .ok_or(ClusterError::UnknownNode(target))?;
        let host = if info.advertise_addr.is_empty() {
            info.address
                .split(':')
                .next()
                .unwrap_or("127.0.0.1")
                .to_string()
        } else {
            info.advertise_addr.clone()
        };
        let port = if info.api_port > 0 {
            info.api_port
        } else {
            3669
        };
        Ok(Some(ContainerPlacement {
            node_id: target,
            api_base_url: format!("http://{host}:{port}"),
            is_self: target == self.node_id,
        }))
    }
}

// ============================================================================
// StaticCluster
// ============================================================================

use std::sync::atomic::{AtomicI64, Ordering};
use tokio::sync::RwLock;

/// Static-membership cluster: no raft, no gossip, no consensus.
///
/// Useful for small deployments (3–5 nodes) where consensus overhead isn't
/// wanted. Leader = lowest-id Ready peer by deterministic election;
/// liveness comes from periodic HTTP probes on `/health`. Existing
/// containers continue running on partitioned nodes — same semantics as
/// every other orchestrator surveyed in the research notes.
pub struct StaticCluster {
    /// This node's id.
    node_id: NodeId,
    /// Per-peer state: id → (`api_addr`, labels, os, `last_seen_epoch_secs`).
    /// Wrapped in `RwLock` because the heartbeat task mutates it.
    peers: Arc<RwLock<HashMap<NodeId, StaticPeer>>>,
    /// Failure threshold in seconds. A peer with `last_seen < now - threshold`
    /// is considered `Unreachable`.
    failure_threshold_secs: u64,
    /// HTTP client for peer dispatch + heartbeats.
    http_client: reqwest::Client,
    /// Shared secret for cluster RPCs.
    internal_token: String,
    /// Local dispatch closure (same shape as the other impls).
    local_dispatch: LocalDispatch,
}

/// Per-peer state stored in `StaticCluster::peers`.
#[derive(Debug, Clone)]
struct StaticPeer {
    api_addr: SocketAddr,
    labels: HashMap<String, String>,
    os: String,
    /// Unix epoch seconds of last successful heartbeat. `AtomicI64` so the
    /// heartbeat task can update it without taking a write lock on `peers`.
    last_seen: Arc<AtomicI64>,
}

/// Spec for a single peer at startup. Translates to `StaticPeer` after
/// `StaticCluster::new` records the initial timestamp.
#[derive(Debug, Clone)]
pub struct StaticPeerSpec {
    pub id: NodeId,
    pub api_addr: SocketAddr,
    pub labels: HashMap<String, String>,
    pub os: String,
}

impl StaticCluster {
    /// Build a static cluster from a config-supplied peer list.
    ///
    /// `peers` MUST include this node (with `id == node_id`). The startup
    /// `last_seen` for every peer is set to now: the heartbeat task will
    /// refresh non-self peers, and self is always Ready by definition.
    ///
    /// Call `start_heartbeats` after construction to spawn the periodic
    /// probe task.
    #[must_use]
    pub fn new<F, Fut>(
        node_id: NodeId,
        peer_specs: Vec<StaticPeerSpec>,
        failure_threshold_secs: u64,
        http_client: reqwest::Client,
        internal_token: String,
        local_dispatch: F,
    ) -> Self
    where
        F: Fn(InternalScaleRequest) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(), ClusterError>> + Send + 'static,
    {
        let now = now_epoch_secs();
        let mut peers = HashMap::new();
        for spec in peer_specs {
            peers.insert(
                spec.id,
                StaticPeer {
                    api_addr: spec.api_addr,
                    labels: spec.labels,
                    os: spec.os,
                    last_seen: Arc::new(AtomicI64::new(now)),
                },
            );
        }
        Self {
            node_id,
            peers: Arc::new(RwLock::new(peers)),
            failure_threshold_secs,
            http_client,
            internal_token,
            local_dispatch: Box::new(move |req| Box::pin(local_dispatch(req))),
        }
    }

    /// Spawn the periodic peer-health probe task.
    ///
    /// Each `interval` the task HEAD-requests `http://<api_addr>/health` on
    /// every non-self peer, updating its `last_seen` on success. Failures
    /// leave the timestamp stale; once `now - last_seen > failure_threshold_secs`,
    /// `state_of` returns `Unreachable`.
    pub fn start_heartbeats(self: Arc<Self>, interval: Duration) {
        let this = Arc::clone(&self);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            ticker.tick().await; // consume immediate tick
            loop {
                ticker.tick().await;
                let probes: Vec<(NodeId, SocketAddr, Arc<AtomicI64>)> = {
                    let peers = this.peers.read().await;
                    peers
                        .iter()
                        .filter(|(id, _)| **id != this.node_id)
                        .map(|(id, p)| (*id, p.api_addr, Arc::clone(&p.last_seen)))
                        .collect()
                };
                for (id, addr, last_seen) in probes {
                    let url = format!("http://{addr}/health");
                    let res = this
                        .http_client
                        .head(&url)
                        .timeout(Duration::from_secs(3))
                        .send()
                        .await;
                    match res {
                        Ok(r) if r.status().is_success() || r.status().as_u16() == 405 => {
                            // 405 (HEAD not allowed) still confirms the
                            // server is reachable — many web frameworks
                            // respond 405 to HEAD on GET-only routes.
                            last_seen.store(now_epoch_secs(), Ordering::SeqCst);
                        }
                        Ok(r) => {
                            tracing::debug!(
                                node_id = id,
                                status = %r.status(),
                                "peer health probe non-success"
                            );
                        }
                        Err(e) => {
                            tracing::debug!(node_id = id, error = %e, "peer health probe failed");
                        }
                    }
                }
            }
        });
    }

    fn state_of(&self, peer: &StaticPeer) -> NodeState {
        let now = now_epoch_secs();
        let last = peer.last_seen.load(Ordering::SeqCst);
        let delta = now.saturating_sub(last).max(0);
        let delta_secs = u64::try_from(delta).unwrap_or(0);
        if delta_secs > self.failure_threshold_secs {
            NodeState::Unreachable
        } else {
            NodeState::Ready
        }
    }

    async fn http_dispatch(
        &self,
        api_addr: SocketAddr,
        req: &InternalScaleRequest,
    ) -> Result<(), ClusterError> {
        let url = format!("http://{api_addr}/api/v1/internal/scale");
        let resp = self
            .http_client
            .post(&url)
            .header("X-ZLayer-Internal-Token", &self.internal_token)
            .json(req)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| ClusterError::Transport(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(ClusterError::HttpStatus { status, body });
        }
        Ok(())
    }
}

fn now_epoch_secs() -> i64 {
    use std::time::UNIX_EPOCH;
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[async_trait]
impl Cluster for StaticCluster {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    async fn is_leader(&self) -> bool {
        // Deterministic: lowest-id Ready peer wins.
        let peers = self.peers.read().await;
        let mut ready: Vec<NodeId> = peers
            .iter()
            .filter(|(id, p)| **id == self.node_id || self.state_of(p) == NodeState::Ready)
            .map(|(id, _)| *id)
            .collect();
        ready.sort_unstable();
        ready.first().copied() == Some(self.node_id)
    }

    async fn leader_addr(&self) -> Option<SocketAddr> {
        let peers = self.peers.read().await;
        let mut candidates: Vec<(NodeId, SocketAddr)> = peers
            .iter()
            .filter(|(id, p)| **id == self.node_id || self.state_of(p) == NodeState::Ready)
            .map(|(id, p)| (*id, p.api_addr))
            .collect();
        candidates.sort_by_key(|(id, _)| *id);
        candidates.first().map(|(_, addr)| *addr)
    }

    async fn list_nodes(&self) -> Vec<NodeRecord> {
        let peers = self.peers.read().await;
        peers
            .iter()
            .map(|(id, p)| {
                let state = if *id == self.node_id {
                    NodeState::Ready
                } else {
                    self.state_of(p)
                };
                NodeRecord {
                    id: *id,
                    api_addr: p.api_addr,
                    labels: p.labels.clone(),
                    os: p.os.clone(),
                    state,
                    last_seen: SystemTime::now(),
                }
            })
            .collect()
    }

    async fn dispatch_scale(
        &self,
        target: NodeId,
        req: InternalScaleRequest,
    ) -> Result<(), ClusterError> {
        if target == self.node_id {
            return (self.local_dispatch)(req).await;
        }
        let api_addr = {
            let peers = self.peers.read().await;
            let peer = peers
                .get(&target)
                .ok_or(ClusterError::UnknownNode(target))?;
            let state = self.state_of(peer);
            if state != NodeState::Ready {
                return Err(ClusterError::NodeNotReady(target, state));
            }
            peer.api_addr
        };
        self.http_dispatch(api_addr, &req).await
    }

    async fn forward_scale(&self, req: InternalScaleRequest) -> Result<(), ClusterError> {
        if self.is_leader().await {
            return (self.local_dispatch)(req).await;
        }
        let leader_addr = self.leader_addr().await.ok_or(ClusterError::NoLeader)?;
        self.http_dispatch(leader_addr, &req).await
    }

    async fn place_container(
        &self,
        spec: &zlayer_spec::ServiceSpec,
    ) -> Result<Option<ContainerPlacement>, ClusterError> {
        let peers = self.peers.read().await;
        let nodes: Vec<crate::placement::NodeState> = peers
            .iter()
            .filter(|(_, p)| self.state_of(p) == NodeState::Ready)
            .map(|(id, p)| node_state_from_os(*id, p.api_addr, &p.os, p.labels.clone()))
            .collect();
        let Some(target) = crate::placement::place_single_container(spec, &nodes) else {
            return Ok(None);
        };
        let api_addr = peers
            .get(&target)
            .map(|p| p.api_addr)
            .ok_or(ClusterError::UnknownNode(target))?;
        Ok(Some(ContainerPlacement {
            node_id: target,
            api_base_url: format!("http://{api_addr}"),
            is_self: target == self.node_id,
        }))
    }
}

// ============================================================================
// WorkerTierCluster (Phase 3 — Nomad-style 1000-worker tier).
// ============================================================================
//
// The control-plane (Raft) keeps the consensus group small (3-7 nodes) and
// workers join as gRPC clients with adaptive-TTL heartbeats. The server-side
// of this Cluster impl wraps RaftCluster (delegates is_leader/list_nodes/etc)
// + owns a `WorkerDispatcherHandle` for pushing assignments down to workers.
// The worker-side is lightweight: only knows how to call the control plane.
//
// gRPC plumbing lives in `worker_dispatcher.rs` (server) and
// `worker_client.rs` (worker, in the agent crate). This file owns the
// Cluster trait shape and routes calls through the right side.

/// What role this node plays in a worker-tier cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerTierRole {
    /// Control-plane node: participates in Raft consensus + serves workers.
    Server,
    /// Worker node: gRPC client only; never enters consensus.
    Worker,
}

/// Server-side worker dispatcher handle. The concrete type is in
/// `worker_dispatcher.rs` (Phase 3.5); this trait keeps the cluster module
/// from depending on Tonic directly.
#[async_trait]
pub trait WorkerDispatcher: Send + Sync + std::fmt::Debug {
    /// Push a scale request to a specific worker by its assigned `node_id`.
    async fn dispatch_to_worker(
        &self,
        target: NodeId,
        req: InternalScaleRequest,
    ) -> Result<(), ClusterError>;

    /// Snapshot of every currently-leased worker. Used by `list_nodes`
    /// alongside the raft-known control-plane nodes.
    async fn known_workers(&self) -> Vec<NodeRecord>;

    /// Total count of currently-leased workers (used by adaptive-TTL math).
    async fn worker_count(&self) -> usize;
}

/// Worker-side handle for talking back to the control plane. Concrete type in
/// `zlayer-agent/src/worker_client.rs` (Phase 3.6).
#[async_trait]
pub trait WorkerClient: Send + Sync + std::fmt::Debug {
    /// Currently-believed leader address (set by the most recent
    /// successful `Register` or `WatchAssignments` reconnect).
    async fn current_leader_addr(&self) -> Option<SocketAddr>;

    /// Best-known peer list (the control plane sends snapshots over
    /// the gRPC stream).
    async fn known_peers(&self) -> Vec<NodeRecord>;

    /// Node-id assigned by the leader during `Register`.
    fn assigned_node_id(&self) -> NodeId;
}

/// `Cluster` impl for worker-tier mode.
///
/// Two variants:
/// - [`WorkerTierMode::Server`] wraps an inner [`RaftCluster`] and owns a
///   [`WorkerDispatcher`] for fan-out to workers. Most `Cluster` calls
///   delegate to the inner `RaftCluster`; `list_nodes` is augmented with the
///   set of worker leases (read via the dispatcher); `dispatch_scale` routes
///   to either an inner-raft peer or a worker depending on the target id.
/// - [`WorkerTierMode::Worker`] holds only a [`WorkerClient`] and `is_leader`
///   always returns false. `dispatch_scale` returns `NotLeader`;
///   `forward_scale` round-trips via the gRPC client to the current leader.
pub enum WorkerTierMode {
    Server {
        inner: Arc<RaftCluster>,
        dispatcher: Arc<dyn WorkerDispatcher>,
    },
    Worker {
        client: Arc<dyn WorkerClient>,
        local_dispatch: Arc<RwLock<Option<LocalDispatch>>>,
    },
}

impl std::fmt::Debug for WorkerTierMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Server { inner, dispatcher } => f
                .debug_struct("WorkerTierMode::Server")
                .field("inner_node_id", &inner.node_id())
                .field("dispatcher", dispatcher)
                .finish(),
            Self::Worker { client, .. } => f
                .debug_struct("WorkerTierMode::Worker")
                .field("client", client)
                .finish_non_exhaustive(),
        }
    }
}

/// See module-level doc on [`WorkerTierMode`].
#[derive(Debug)]
pub struct WorkerTierCluster {
    mode: WorkerTierMode,
    /// What role this node plays. Cached so `node_id()` doesn't have to
    /// pattern-match every call.
    role: WorkerTierRole,
}

impl WorkerTierCluster {
    /// Construct a server-side worker-tier cluster wrapping the provided
    /// `RaftCluster` and `WorkerDispatcher`.
    #[must_use]
    pub fn server(inner: Arc<RaftCluster>, dispatcher: Arc<dyn WorkerDispatcher>) -> Self {
        Self {
            mode: WorkerTierMode::Server { inner, dispatcher },
            role: WorkerTierRole::Server,
        }
    }

    /// Construct a worker-side worker-tier cluster (no raft, only the gRPC
    /// client back to the control plane).
    #[must_use]
    pub fn worker(client: Arc<dyn WorkerClient>) -> Self {
        Self {
            mode: WorkerTierMode::Worker {
                client,
                local_dispatch: Arc::new(RwLock::new(None)),
            },
            role: WorkerTierRole::Worker,
        }
    }

    /// Set the local-dispatch callback (used by the worker side when the
    /// control plane pushes an assignment that targets THIS worker —
    /// instead of bouncing through gRPC again, run it locally).
    pub async fn set_local_dispatch(&self, dispatch: LocalDispatch) {
        if let WorkerTierMode::Worker { local_dispatch, .. } = &self.mode {
            *local_dispatch.write().await = Some(dispatch);
        }
    }

    /// What role this node plays.
    #[must_use]
    pub fn role(&self) -> WorkerTierRole {
        self.role
    }

    /// Server-side: the inner `RaftCluster` (for callers that need direct
    /// raft access — e.g. the worker dispatcher when it grants a lease via
    /// the raft FSM). None on worker-side.
    #[must_use]
    pub fn raft(&self) -> Option<&Arc<RaftCluster>> {
        match &self.mode {
            WorkerTierMode::Server { inner, .. } => Some(inner),
            WorkerTierMode::Worker { .. } => None,
        }
    }

    /// Server-side: the worker dispatcher. None on worker-side.
    #[must_use]
    pub fn dispatcher(&self) -> Option<&Arc<dyn WorkerDispatcher>> {
        match &self.mode {
            WorkerTierMode::Server { dispatcher, .. } => Some(dispatcher),
            WorkerTierMode::Worker { .. } => None,
        }
    }

    /// Worker-side: the gRPC client. None on server-side.
    #[must_use]
    pub fn client(&self) -> Option<&Arc<dyn WorkerClient>> {
        match &self.mode {
            WorkerTierMode::Server { .. } => None,
            WorkerTierMode::Worker { client, .. } => Some(client),
        }
    }
}

#[async_trait]
impl Cluster for WorkerTierCluster {
    fn node_id(&self) -> NodeId {
        match &self.mode {
            WorkerTierMode::Server { inner, .. } => inner.node_id(),
            WorkerTierMode::Worker { client, .. } => client.assigned_node_id(),
        }
    }

    async fn is_leader(&self) -> bool {
        match &self.mode {
            WorkerTierMode::Server { inner, .. } => inner.is_leader().await,
            WorkerTierMode::Worker { .. } => false,
        }
    }

    async fn leader_addr(&self) -> Option<SocketAddr> {
        match &self.mode {
            WorkerTierMode::Server { inner, .. } => inner.leader_addr().await,
            WorkerTierMode::Worker { client, .. } => client.current_leader_addr().await,
        }
    }

    async fn list_nodes(&self) -> Vec<NodeRecord> {
        match &self.mode {
            WorkerTierMode::Server { inner, dispatcher } => {
                // Control-plane nodes (from raft) + worker leases (from
                // dispatcher). Deduplicate by NodeId — a worker that
                // somehow appears in both registries shows up once.
                let mut all = inner.list_nodes().await;
                let known_workers = dispatcher.known_workers().await;
                let existing: std::collections::HashSet<NodeId> =
                    all.iter().map(|n| n.id).collect();
                all.extend(
                    known_workers
                        .into_iter()
                        .filter(|n| !existing.contains(&n.id)),
                );
                all
            }
            WorkerTierMode::Worker { client, .. } => client.known_peers().await,
        }
    }

    async fn dispatch_scale(
        &self,
        target: NodeId,
        req: InternalScaleRequest,
    ) -> Result<(), ClusterError> {
        match &self.mode {
            WorkerTierMode::Server { inner, dispatcher } => {
                // Decide: is `target` a raft member or a worker?
                if inner.knows_node(target).await {
                    inner.dispatch_scale(target, req).await
                } else {
                    dispatcher.dispatch_to_worker(target, req).await
                }
            }
            WorkerTierMode::Worker { .. } => {
                // Workers never originate dispatch. The control plane pushes
                // assignments to them via WatchAssignments.
                Err(ClusterError::NotLeader(None))
            }
        }
    }

    async fn forward_scale(&self, req: InternalScaleRequest) -> Result<(), ClusterError> {
        match &self.mode {
            WorkerTierMode::Server { inner, .. } => inner.forward_scale(req).await,
            WorkerTierMode::Worker {
                client,
                local_dispatch,
            } => {
                // Worker received a scale request directly. The protocol is
                // "control plane pushes via WatchAssignments", so anything
                // arriving here from a local caller (CLI invoked on the
                // worker, etc.) needs to be forwarded to the current leader.
                // Worker-side forwarding goes through the gRPC client — but
                // the gRPC client may not have a generic "forward scale"
                // call. Until P3.6 wires that, return NotLeader so the
                // caller knows to talk to the control plane directly.
                //
                // Once P3.6 lands a generic forward path on `WorkerClient`,
                // call it here. For now, surface the leader as "known"
                // (placeholder node id 0) so the caller can re-issue the
                // request itself.
                let _ = local_dispatch;
                Err(ClusterError::NotLeader(
                    client.current_leader_addr().await.map(|_| 0u64),
                ))
            }
        }
    }

    async fn place_container(
        &self,
        spec: &zlayer_spec::ServiceSpec,
    ) -> Result<Option<ContainerPlacement>, ClusterError> {
        match &self.mode {
            WorkerTierMode::Server { inner, .. } => inner.place_container(spec).await,
            // Worker-side nodes don't make placement decisions; the control
            // plane does. Surface "no placement" so the caller forwards.
            WorkerTierMode::Worker { .. } => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn dummy_addr() -> SocketAddr {
        "127.0.0.1:3669".parse().unwrap()
    }

    #[tokio::test]
    async fn single_node_is_leader() {
        let cluster = SingleNodeCluster::new(1, dummy_addr(), "linux", |_| async { Ok(()) });
        assert!(cluster.is_leader().await);
        assert_eq!(cluster.node_id(), 1);
        assert_eq!(cluster.leader_addr().await, Some(dummy_addr()));
    }

    #[tokio::test]
    async fn single_node_lists_self() {
        let cluster = SingleNodeCluster::new(1, dummy_addr(), "linux", |_| async { Ok(()) });
        let nodes = cluster.list_nodes().await;
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, 1);
        assert_eq!(nodes[0].state, NodeState::Ready);
    }

    #[tokio::test]
    async fn single_node_dispatch_local_only() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let cluster = SingleNodeCluster::new(7, dummy_addr(), "linux", move |_req| {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        });

        cluster
            .dispatch_scale(7, InternalScaleRequest::new("svc", 3))
            .await
            .expect("local dispatch ok");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let err = cluster
            .dispatch_scale(99, InternalScaleRequest::new("svc", 1))
            .await
            .unwrap_err();
        assert!(matches!(err, ClusterError::UnknownNode(99)));
    }

    #[tokio::test]
    async fn forward_scale_falls_through_locally() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let cluster = SingleNodeCluster::new(1, dummy_addr(), "linux", move |_req| {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        });
        cluster
            .forward_scale(InternalScaleRequest::new("svc", 2))
            .await
            .expect("forward ok");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn static_cluster_sole_member_is_leader() {
        let me = StaticPeerSpec {
            id: 1,
            api_addr: dummy_addr(),
            labels: HashMap::new(),
            os: "linux".to_string(),
        };
        let cluster = StaticCluster::new(
            1,
            vec![me],
            15,
            reqwest::Client::new(),
            "tok".to_string(),
            |_| async { Ok(()) },
        );
        assert!(cluster.is_leader().await);
        assert_eq!(cluster.list_nodes().await.len(), 1);
    }

    #[tokio::test]
    async fn static_cluster_lowest_id_wins() {
        // Three peers; we're id=2. id=1 is ready (fresh last_seen).
        let peers = vec![
            StaticPeerSpec {
                id: 1,
                api_addr: "127.0.0.1:3001".parse().unwrap(),
                labels: HashMap::new(),
                os: "linux".to_string(),
            },
            StaticPeerSpec {
                id: 2,
                api_addr: "127.0.0.1:3002".parse().unwrap(),
                labels: HashMap::new(),
                os: "linux".to_string(),
            },
            StaticPeerSpec {
                id: 3,
                api_addr: "127.0.0.1:3003".parse().unwrap(),
                labels: HashMap::new(),
                os: "linux".to_string(),
            },
        ];
        let cluster = StaticCluster::new(
            2,
            peers,
            15,
            reqwest::Client::new(),
            "tok".to_string(),
            |_| async { Ok(()) },
        );
        // node 1 should be leader (lowest id, default-ready)
        assert!(!cluster.is_leader().await);
        assert_eq!(
            cluster.leader_addr().await,
            Some("127.0.0.1:3001".parse().unwrap())
        );
    }

    // ------------------------------------------------------------------------
    // WorkerTierCluster tests (Phase 3.4)
    // ------------------------------------------------------------------------

    #[derive(Debug)]
    #[allow(dead_code)] // server-mode wiring tests land in P3.5
    struct WorkerTierStubDispatcher {
        node_id: NodeId,
    }

    #[async_trait]
    impl WorkerDispatcher for WorkerTierStubDispatcher {
        async fn dispatch_to_worker(
            &self,
            _target: NodeId,
            _req: InternalScaleRequest,
        ) -> Result<(), ClusterError> {
            Ok(())
        }
        async fn known_workers(&self) -> Vec<NodeRecord> {
            vec![NodeRecord {
                id: self.node_id,
                api_addr: "127.0.0.1:9999".parse().unwrap(),
                labels: HashMap::new(),
                os: "linux".into(),
                state: NodeState::Ready,
                last_seen: SystemTime::now(),
            }]
        }
        async fn worker_count(&self) -> usize {
            1
        }
    }

    #[derive(Debug)]
    struct WorkerTierStubClient {
        node_id: NodeId,
        leader: Option<SocketAddr>,
    }

    #[async_trait]
    impl WorkerClient for WorkerTierStubClient {
        async fn current_leader_addr(&self) -> Option<SocketAddr> {
            self.leader
        }
        async fn known_peers(&self) -> Vec<NodeRecord> {
            vec![]
        }
        fn assigned_node_id(&self) -> NodeId {
            self.node_id
        }
    }

    #[tokio::test]
    async fn worker_tier_worker_mode_is_never_leader() {
        let client = Arc::new(WorkerTierStubClient {
            node_id: 42,
            leader: Some("10.0.0.1:3669".parse().unwrap()),
        });
        let cluster = WorkerTierCluster::worker(client);
        assert!(!cluster.is_leader().await);
        assert_eq!(cluster.node_id(), 42);
        assert_eq!(
            cluster.leader_addr().await,
            Some("10.0.0.1:3669".parse().unwrap())
        );
        assert_eq!(cluster.role(), WorkerTierRole::Worker);
    }

    #[tokio::test]
    async fn worker_tier_dispatch_from_worker_returns_not_leader() {
        let client = Arc::new(WorkerTierStubClient {
            node_id: 42,
            leader: None,
        });
        let cluster = WorkerTierCluster::worker(client);
        let req = InternalScaleRequest::new("svc", 1);
        let err = cluster.dispatch_scale(1, req).await.unwrap_err();
        assert!(matches!(err, ClusterError::NotLeader(_)));
    }
}
