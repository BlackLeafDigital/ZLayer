//! OpenRaft distributed coordination for ZLayer scheduler
//!
//! Provides consensus-based service state management across nodes.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::io::Cursor;
use std::sync::Arc;

use openraft::error::{
    Fatal, InstallSnapshotError, RPCError, RaftError, ReplicationClosed, StreamingError,
    Unreachable,
};
use openraft::network::{RPCOption, RaftNetwork, RaftNetworkFactory};
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest, InstallSnapshotResponse,
    SnapshotResponse, VoteRequest, VoteResponse,
};
use openraft::storage::Adaptor;
use openraft::{
    BasicNode, Config, Entry, LogId, OptionalSend, Raft, RaftTypeConfig, Snapshot,
    StoredMembership, Vote,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::error::{Result, SchedulerError};
use crate::raft_network::RaftHttpClient;
use crate::raft_storage::MemStore;

/// Node ID type (u64 for simplicity)
pub type NodeId = u64;

/// OpenRaft type configuration for ZLayer
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Ord, PartialOrd)]
pub struct TypeConfig;

impl RaftTypeConfig for TypeConfig {
    type D = Request;
    type R = Response;
    type NodeId = NodeId;
    type Node = BasicNode;
    type Entry = Entry<TypeConfig>;
    type SnapshotData = Cursor<Vec<u8>>;
    type Responder = openraft::impls::OneshotResponder<TypeConfig>;
    type AsyncRuntime = openraft::TokioRuntime;
}

/// Raft request types (state machine commands)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    /// Update service state
    UpdateServiceState {
        service_name: String,
        state: ServiceState,
    },
    /// Record scaling decision
    RecordScaleEvent {
        service_name: String,
        from_replicas: u32,
        to_replicas: u32,
        reason: String,
        timestamp: u64,
    },
    /// Register a new node
    RegisterNode { node_id: NodeId, address: String },
    /// Deregister a node
    DeregisterNode { node_id: NodeId },
    /// Update service assignment to nodes
    UpdateServiceAssignment {
        service_name: String,
        node_ids: Vec<NodeId>,
    },
}

/// Raft response types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    /// Success with optional data
    Success { data: Option<String> },
    /// Error response
    Error { message: String },
}

/// Service state tracked in Raft
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServiceState {
    /// Current number of replicas
    pub current_replicas: u32,
    /// Desired number of replicas
    pub desired_replicas: u32,
    /// Minimum replicas from spec
    pub min_replicas: u32,
    /// Maximum replicas from spec
    pub max_replicas: u32,
    /// Service health status
    pub health_status: HealthStatus,
    /// Last scale event timestamp (unix millis)
    pub last_scale_time: Option<u64>,
    /// Nodes running this service
    pub assigned_nodes: Vec<NodeId>,
}

/// Health status for services
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum HealthStatus {
    #[default]
    Unknown,
    Healthy,
    Degraded,
    Unhealthy,
}

/// Scale event record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScaleEvent {
    pub service_name: String,
    pub from_replicas: u32,
    pub to_replicas: u32,
    pub reason: String,
    pub timestamp: u64,
}

/// Cluster state (the state machine)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClusterState {
    /// Last applied log index
    pub last_applied_log: Option<LogId<NodeId>>,
    /// Last membership config
    pub last_membership: StoredMembership<NodeId, BasicNode>,
    /// Service states
    pub services: HashMap<String, ServiceState>,
    /// Node registry
    pub nodes: HashMap<NodeId, NodeInfo>,
    /// Recent scale events (ring buffer, keep last 100)
    pub scale_events: Vec<ScaleEvent>,
}

/// Node information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub node_id: NodeId,
    pub address: String,
    pub registered_at: u64,
    pub last_heartbeat: u64,
    /// Detected GPUs on this node
    #[serde(default)]
    pub gpus: Vec<GpuInfoSummary>,
}

/// Summary of a GPU on a node, stored in Raft cluster state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GpuInfoSummary {
    /// Vendor: "nvidia", "amd", "intel", or "unknown"
    pub vendor: String,
    /// Model name (e.g., "NVIDIA A100-SXM4-80GB")
    pub model: String,
    /// VRAM in MB
    pub memory_mb: u64,
}

impl ClusterState {
    /// Create a new empty cluster state
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a request to the state machine
    pub fn apply(&mut self, request: &Request) -> Response {
        match request {
            Request::UpdateServiceState {
                service_name,
                state,
            } => {
                self.services.insert(service_name.clone(), state.clone());
                Response::Success { data: None }
            }
            Request::RecordScaleEvent {
                service_name,
                from_replicas,
                to_replicas,
                reason,
                timestamp,
            } => {
                let event = ScaleEvent {
                    service_name: service_name.clone(),
                    from_replicas: *from_replicas,
                    to_replicas: *to_replicas,
                    reason: reason.clone(),
                    timestamp: *timestamp,
                };

                // Keep last 100 events
                self.scale_events.push(event);
                if self.scale_events.len() > 100 {
                    self.scale_events.remove(0);
                }

                // Update last scale time on service
                if let Some(svc) = self.services.get_mut(service_name) {
                    svc.last_scale_time = Some(*timestamp);
                    svc.current_replicas = *to_replicas;
                }

                Response::Success { data: None }
            }
            Request::RegisterNode { node_id, address } => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64;

                self.nodes.insert(
                    *node_id,
                    NodeInfo {
                        node_id: *node_id,
                        address: address.clone(),
                        registered_at: now,
                        last_heartbeat: now,
                        gpus: Vec::new(),
                    },
                );

                Response::Success { data: None }
            }
            Request::DeregisterNode { node_id } => {
                self.nodes.remove(node_id);

                // Remove from all service assignments
                for svc in self.services.values_mut() {
                    svc.assigned_nodes.retain(|n| n != node_id);
                }

                Response::Success { data: None }
            }
            Request::UpdateServiceAssignment {
                service_name,
                node_ids,
            } => {
                if let Some(svc) = self.services.get_mut(service_name) {
                    svc.assigned_nodes = node_ids.clone();
                    Response::Success { data: None }
                } else {
                    Response::Error {
                        message: format!("Service not found: {}", service_name),
                    }
                }
            }
        }
    }

    /// Get service state
    pub fn get_service(&self, name: &str) -> Option<&ServiceState> {
        self.services.get(name)
    }

    /// Get all services
    pub fn get_services(&self) -> &HashMap<String, ServiceState> {
        &self.services
    }

    /// Get node info
    pub fn get_node(&self, node_id: NodeId) -> Option<&NodeInfo> {
        self.nodes.get(&node_id)
    }

    /// Get recent scale events
    pub fn get_scale_events(&self, limit: usize) -> &[ScaleEvent] {
        let start = self.scale_events.len().saturating_sub(limit);
        &self.scale_events[start..]
    }
}

/// Type alias for the configured Raft instance
pub type ZLayerRaft = Raft<TypeConfig>;

// =============================================================================
// Raft Configuration
// =============================================================================

/// Configuration for the Raft coordinator
#[derive(Debug, Clone)]
pub struct RaftConfig {
    /// This node's ID
    pub node_id: NodeId,
    /// This node's address for Raft communication
    pub address: String,
    /// Heartbeat interval in milliseconds
    pub heartbeat_interval_ms: u64,
    /// Election timeout range in milliseconds (min, max)
    pub election_timeout_ms: (u64, u64),
    /// RPC timeout in milliseconds
    pub rpc_timeout_ms: u64,
    /// Raft API port (default 9090)
    pub raft_port: u16,
}

impl Default for RaftConfig {
    fn default() -> Self {
        Self {
            node_id: 1,
            address: "127.0.0.1:9000".to_string(),
            heartbeat_interval_ms: 150,
            election_timeout_ms: (300, 600),
            rpc_timeout_ms: 5000,
            raft_port: 9090,
        }
    }
}

// =============================================================================
// Network Layer
// =============================================================================

/// Network implementation for Raft RPCs
///
/// Uses HTTP-based RPC client to communicate between nodes.
pub struct ZLayerNetwork {
    /// Known peer addresses
    peers: Arc<RwLock<HashMap<NodeId, String>>>,
    /// Shared HTTP client for making RPC calls
    client: Arc<RaftHttpClient>,
}

impl ZLayerNetwork {
    /// Create a new network layer with default timeout (5 seconds)
    pub fn new() -> Self {
        Self::with_timeout(5000)
    }

    /// Create a new network layer with specified timeout in milliseconds
    pub fn with_timeout(timeout_ms: u64) -> Self {
        Self {
            peers: Arc::new(RwLock::new(HashMap::new())),
            client: Arc::new(RaftHttpClient::new(timeout_ms)),
        }
    }

    /// Add a peer
    pub async fn add_peer(&self, node_id: NodeId, address: String) {
        let mut peers = self.peers.write().await;
        peers.insert(node_id, address);
    }

    /// Remove a peer
    pub async fn remove_peer(&self, node_id: NodeId) {
        let mut peers = self.peers.write().await;
        peers.remove(&node_id);
    }

    /// Get all known peers
    pub async fn peers(&self) -> HashMap<NodeId, String> {
        self.peers.read().await.clone()
    }
}

impl Default for ZLayerNetwork {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for ZLayerNetwork {
    fn clone(&self) -> Self {
        Self {
            peers: Arc::clone(&self.peers),
            client: Arc::clone(&self.client),
        }
    }
}

// OpenRaft network factory trait implementation
impl RaftNetworkFactory<TypeConfig> for ZLayerNetwork {
    type Network = ZLayerNetworkConnection;

    async fn new_client(&mut self, _target: NodeId, node: &BasicNode) -> Self::Network {
        ZLayerNetworkConnection {
            target_addr: node.addr.clone(),
            client: Arc::clone(&self.client),
        }
    }
}

/// A connection to a single Raft peer
pub struct ZLayerNetworkConnection {
    target_addr: String,
    client: Arc<RaftHttpClient>,
}

impl RaftNetwork<TypeConfig> for ZLayerNetworkConnection {
    async fn append_entries(
        &mut self,
        rpc: AppendEntriesRequest<TypeConfig>,
        _option: RPCOption,
    ) -> std::result::Result<
        AppendEntriesResponse<NodeId>,
        RPCError<NodeId, BasicNode, RaftError<NodeId>>,
    > {
        debug!(target = %self.target_addr, "Sending append_entries RPC");
        self.client
            .append_entries(&self.target_addr, rpc)
            .await
            .map_err(|e| {
                RPCError::Unreachable(Unreachable::new(&std::io::Error::other(e.to_string())))
            })
    }

    async fn install_snapshot(
        &mut self,
        rpc: InstallSnapshotRequest<TypeConfig>,
        _option: RPCOption,
    ) -> std::result::Result<
        InstallSnapshotResponse<NodeId>,
        RPCError<NodeId, BasicNode, RaftError<NodeId, InstallSnapshotError>>,
    > {
        debug!(target = %self.target_addr, "Sending install_snapshot RPC");
        self.client
            .install_snapshot(&self.target_addr, rpc)
            .await
            .map_err(|e| {
                RPCError::Unreachable(Unreachable::new(&std::io::Error::other(e.to_string())))
            })
    }

    async fn vote(
        &mut self,
        rpc: VoteRequest<NodeId>,
        _option: RPCOption,
    ) -> std::result::Result<VoteResponse<NodeId>, RPCError<NodeId, BasicNode, RaftError<NodeId>>>
    {
        debug!(target = %self.target_addr, "Sending vote RPC");
        self.client.vote(&self.target_addr, rpc).await.map_err(|e| {
            RPCError::Unreachable(Unreachable::new(&std::io::Error::other(e.to_string())))
        })
    }

    async fn full_snapshot(
        &mut self,
        vote: Vote<NodeId>,
        snapshot: Snapshot<TypeConfig>,
        _cancel: impl Future<Output = ReplicationClosed> + OptionalSend + 'static,
        _option: RPCOption,
    ) -> std::result::Result<SnapshotResponse<NodeId>, StreamingError<TypeConfig, Fatal<NodeId>>>
    {
        debug!(target = %self.target_addr, "Sending full_snapshot RPC");

        // Read snapshot data into a buffer
        let mut snapshot_data = Vec::new();
        let mut snapshot_reader = snapshot.snapshot;
        std::io::Read::read_to_end(&mut snapshot_reader, &mut snapshot_data).map_err(|e| {
            StreamingError::Unreachable(Unreachable::new(&std::io::Error::other(format!(
                "Failed to read snapshot: {}",
                e
            ))))
        })?;

        self.client
            .full_snapshot(&self.target_addr, vote, snapshot_data)
            .await
            .map_err(|e| StreamingError::Unreachable(Unreachable::new(&std::io::Error::other(e))))
    }
}

// =============================================================================
// Raft Coordinator
// =============================================================================

/// Type alias for the log store adaptor (wraps MemStore for v2 API)
pub type RaftLogStore = Adaptor<TypeConfig, MemStore>;

/// Type alias for the state machine adaptor (wraps MemStore for v2 API)
pub type RaftStateMachine = Adaptor<TypeConfig, MemStore>;

/// High-level coordinator for Raft consensus
///
/// Wraps the OpenRaft instance and provides a simpler API for:
/// - Bootstrapping a new cluster
/// - Joining an existing cluster
/// - Proposing state changes
/// - Reading cluster state
pub struct RaftCoordinator {
    /// OpenRaft instance
    raft: ZLayerRaft,
    /// Log store (adaptor over MemStore)
    log_store: RaftLogStore,
    /// Configuration
    config: RaftConfig,
}

impl RaftCoordinator {
    /// Create a new Raft coordinator
    pub async fn new(config: RaftConfig) -> Result<Self> {
        let raft_config = Config {
            cluster_name: "zlayer".to_string(),
            heartbeat_interval: config.heartbeat_interval_ms,
            election_timeout_min: config.election_timeout_ms.0,
            election_timeout_max: config.election_timeout_ms.1,
            ..Default::default()
        };

        let raft_config =
            Arc::new(raft_config.validate().map_err(|e| {
                SchedulerError::InvalidConfig(format!("Invalid Raft config: {}", e))
            })?);

        let storage = MemStore::new();
        let network = ZLayerNetwork::with_timeout(config.rpc_timeout_ms);

        // Create adaptors for log storage and state machine from the combined storage
        let (log_store, state_machine) = Adaptor::new(storage);

        let raft = Raft::new(
            config.node_id,
            raft_config,
            network,
            log_store.clone(),
            state_machine,
        )
        .await
        .map_err(|e| SchedulerError::Raft(e.to_string()))?;

        info!(node_id = config.node_id, "Created Raft coordinator");

        Ok(Self {
            raft,
            log_store,
            config,
        })
    }

    /// Bootstrap a new single-node cluster
    ///
    /// This should only be called once when creating a new cluster.
    pub async fn bootstrap(&self) -> Result<()> {
        let mut members = BTreeMap::new();
        members.insert(
            self.config.node_id,
            BasicNode {
                addr: self.config.address.clone(),
            },
        );

        self.raft
            .initialize(members)
            .await
            .map_err(|e| SchedulerError::Raft(format!("Bootstrap failed: {}", e)))?;

        info!(
            node_id = self.config.node_id,
            "Bootstrapped single-node cluster"
        );
        Ok(())
    }

    /// Check if this node is the current leader
    pub fn is_leader(&self) -> bool {
        let metrics = self.raft.metrics().borrow().clone();
        metrics.current_leader == Some(self.config.node_id)
    }

    /// Get the current leader's node ID
    pub fn leader_id(&self) -> Option<NodeId> {
        self.raft.metrics().borrow().current_leader
    }

    /// Propose a state change (must be leader)
    pub async fn propose(&self, request: Request) -> Result<Response> {
        let result = self
            .raft
            .client_write(request)
            .await
            .map_err(|e| SchedulerError::Raft(format!("Failed to propose: {}", e)))?;

        Ok(result.data)
    }

    /// Read current cluster state
    pub async fn read_state(&self) -> ClusterState {
        // Access the underlying storage through the adaptor
        let storage = self.log_store.storage().await;
        let sm = storage.state_machine();
        let sm = sm.read().await;
        sm.state.clone()
    }

    /// Get service state
    pub async fn get_service(&self, name: &str) -> Option<ServiceState> {
        let state = self.read_state().await;
        state.services.get(name).cloned()
    }

    /// Update service state (proposes to Raft)
    pub async fn update_service(&self, name: String, state: ServiceState) -> Result<()> {
        self.propose(Request::UpdateServiceState {
            service_name: name,
            state,
        })
        .await?;
        Ok(())
    }

    /// Record a scale event
    pub async fn record_scale_event(
        &self,
        service_name: String,
        from_replicas: u32,
        to_replicas: u32,
        reason: String,
    ) -> Result<()> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        self.propose(Request::RecordScaleEvent {
            service_name,
            from_replicas,
            to_replicas,
            reason,
            timestamp,
        })
        .await?;
        Ok(())
    }

    /// Get Raft metrics
    pub fn metrics(&self) -> openraft::RaftMetrics<NodeId, BasicNode> {
        self.raft.metrics().borrow().clone()
    }

    /// Shutdown the Raft node
    pub async fn shutdown(&self) -> Result<()> {
        self.raft
            .shutdown()
            .await
            .map_err(|e| SchedulerError::Raft(format!("Shutdown failed: {}", e)))?;
        info!("Raft coordinator shut down");
        Ok(())
    }

    /// Get a reference to the underlying Raft instance
    ///
    /// This is used by network handlers to forward RPCs to the Raft node.
    pub fn raft_handle(&self) -> &ZLayerRaft {
        &self.raft
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cluster_state_service_operations() {
        let mut state = ClusterState::new();

        let service_state = ServiceState {
            current_replicas: 2,
            desired_replicas: 3,
            min_replicas: 1,
            max_replicas: 10,
            health_status: HealthStatus::Healthy,
            last_scale_time: None,
            assigned_nodes: vec![1, 2],
        };

        state.apply(&Request::UpdateServiceState {
            service_name: "api".to_string(),
            state: service_state,
        });

        let svc = state.get_service("api").unwrap();
        assert_eq!(svc.current_replicas, 2);
        assert_eq!(svc.assigned_nodes, vec![1, 2]);
    }

    #[test]
    fn test_scale_event_recording() {
        let mut state = ClusterState::new();

        // Add service first
        state.apply(&Request::UpdateServiceState {
            service_name: "api".to_string(),
            state: ServiceState {
                current_replicas: 2,
                ..Default::default()
            },
        });

        // Record scale event
        state.apply(&Request::RecordScaleEvent {
            service_name: "api".to_string(),
            from_replicas: 2,
            to_replicas: 4,
            reason: "High CPU".to_string(),
            timestamp: 1234567890,
        });

        let events = state.get_scale_events(10);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].to_replicas, 4);

        // Service should be updated
        let svc = state.get_service("api").unwrap();
        assert_eq!(svc.current_replicas, 4);
        assert_eq!(svc.last_scale_time, Some(1234567890));
    }

    #[test]
    fn test_node_registration() {
        let mut state = ClusterState::new();

        state.apply(&Request::RegisterNode {
            node_id: 1,
            address: "192.168.1.1:8000".to_string(),
        });

        let node = state.get_node(1).unwrap();
        assert_eq!(node.address, "192.168.1.1:8000");
    }
}
