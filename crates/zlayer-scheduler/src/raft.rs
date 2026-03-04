//! `OpenRaft` distributed coordination for `ZLayer` scheduler
//!
//! Provides consensus-based service state management across nodes.
//! Uses `zlayer-consensus` for storage, network, and Raft RPC plumbing.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use openraft::{BasicNode, Raft};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::info;
use utoipa::ToSchema;
use zlayer_consensus::network::http_client::HttpNetwork;
use zlayer_consensus::storage::mem_store::{MemLogStore, MemStateMachine, SmData};
use zlayer_consensus::{ConsensusConfig, ConsensusNode, ConsensusNodeBuilder};

use crate::error::{Result, SchedulerError};

/// Node ID type (u64 for simplicity)
pub type NodeId = u64;

// OpenRaft type configuration for ZLayer scheduler.
// Uses `declare_raft_types!` which automatically provides v2-compatible
// Entry, SnapshotData, Responder, and AsyncRuntime.
openraft::declare_raft_types!(
    pub TypeConfig:
        D = Request,
        R = Response,
);

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
    RegisterNode {
        node_id: NodeId,
        address: String,
        #[serde(default)]
        wg_public_key: String,
        #[serde(default)]
        overlay_ip: String,
        #[serde(default)]
        overlay_port: u16,
        #[serde(default)]
        advertise_addr: String,
        #[serde(default)]
        api_port: u16,
        #[serde(default)]
        cpu_total: f64,
        #[serde(default)]
        memory_total: u64,
        #[serde(default)]
        disk_total: u64,
        #[serde(default)]
        gpus: Vec<GpuInfoSummary>,
    },
    /// Update node heartbeat with current resource usage
    UpdateNodeHeartbeat {
        node_id: NodeId,
        timestamp: u64,
        cpu_used: f64,
        memory_used: u64,
        disk_used: u64,
    },
    /// Update node status (ready/draining/dead)
    UpdateNodeStatus { node_id: NodeId, status: String },
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

impl Default for Response {
    fn default() -> Self {
        Response::Success { data: None }
    }
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

/// Cluster state (the Raft state machine application state).
///
/// This is the `S` generic parameter to `MemStateMachine<TypeConfig, S, F>`.
/// It only contains application-level fields -- the Raft bookkeeping
/// (`last_applied_log`, `last_membership`) is managed by `SmData` inside the
/// consensus crate.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClusterState {
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
    /// `WireGuard` public key for overlay networking
    #[serde(default)]
    pub wg_public_key: String,
    /// Overlay network IP assigned to this node
    #[serde(default)]
    pub overlay_ip: String,
    /// `WireGuard` overlay port
    #[serde(default)]
    pub overlay_port: u16,
    /// Advertise address (public IP) for this node
    #[serde(default)]
    pub advertise_addr: String,
    /// API server port
    #[serde(default)]
    pub api_port: u16,
    /// Total CPU cores on this node
    #[serde(default)]
    pub cpu_total: f64,
    /// Total memory in bytes
    #[serde(default)]
    pub memory_total: u64,
    /// Total disk in bytes
    #[serde(default)]
    pub disk_total: u64,
    /// Current CPU usage (cores)
    #[serde(default)]
    pub cpu_used: f64,
    /// Current memory usage in bytes
    #[serde(default)]
    pub memory_used: u64,
    /// Current disk usage in bytes
    #[serde(default)]
    pub disk_used: u64,
    /// Node status: "ready", "draining", or "dead"
    #[serde(default = "default_node_status")]
    pub status: String,
}

fn default_node_status() -> String {
    "ready".to_string()
}

/// Summary of a GPU on a node, stored in Raft cluster state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct GpuInfoSummary {
    /// Vendor: "nvidia", "amd", "intel", or "unknown"
    pub vendor: String,
    /// Model name (e.g., "NVIDIA A100-SXM4-80GB")
    pub model: String,
    /// VRAM in MB
    pub memory_mb: u64,
}

/// Parameters for adding a new member to the Raft cluster.
///
/// Bundles the node identity, Raft address, and overlay networking metadata
/// into a single struct to keep `add_member()` signatures clean.
#[derive(Debug, Clone)]
pub struct AddMemberParams {
    /// Raft node ID for the new member
    pub node_id: u64,
    /// Raft RPC address (e.g., "10.0.0.2:9000")
    pub addr: String,
    /// `WireGuard` public key
    pub wg_public_key: String,
    /// Assigned overlay IP
    pub overlay_ip: String,
    /// `WireGuard` overlay port
    pub overlay_port: u16,
    /// Public advertise address (IP)
    pub advertise_addr: String,
    /// API server port
    pub api_port: u16,
    /// Total CPU cores on this node
    pub cpu_total: f64,
    /// Total memory in bytes
    pub memory_total: u64,
    /// Total disk in bytes
    pub disk_total: u64,
    /// Detected GPUs on this node
    pub gpus: Vec<GpuInfoSummary>,
}

impl ClusterState {
    /// Create a new empty cluster state
    #[must_use]
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
            Request::RegisterNode {
                node_id,
                address,
                wg_public_key,
                overlay_ip,
                overlay_port,
                advertise_addr,
                api_port,
                cpu_total,
                memory_total,
                disk_total,
                gpus,
            } => {
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
                        gpus: gpus.clone(),
                        wg_public_key: wg_public_key.clone(),
                        overlay_ip: overlay_ip.clone(),
                        overlay_port: *overlay_port,
                        advertise_addr: advertise_addr.clone(),
                        api_port: *api_port,
                        cpu_total: *cpu_total,
                        memory_total: *memory_total,
                        disk_total: *disk_total,
                        cpu_used: 0.0,
                        memory_used: 0,
                        disk_used: 0,
                        status: "ready".to_string(),
                    },
                );

                Response::Success { data: None }
            }
            Request::UpdateNodeHeartbeat {
                node_id,
                timestamp,
                cpu_used,
                memory_used,
                disk_used,
            } => {
                if let Some(node) = self.nodes.get_mut(node_id) {
                    node.last_heartbeat = *timestamp;
                    node.cpu_used = *cpu_used;
                    node.memory_used = *memory_used;
                    node.disk_used = *disk_used;
                }
                Response::Success { data: None }
            }
            Request::UpdateNodeStatus { node_id, status } => {
                if let Some(node) = self.nodes.get_mut(node_id) {
                    node.status = status.clone();
                }
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
                        message: format!("Service not found: {service_name}"),
                    }
                }
            }
        }
    }

    /// Get service state
    #[must_use]
    pub fn get_service(&self, name: &str) -> Option<&ServiceState> {
        self.services.get(name)
    }

    /// Get all services
    #[must_use]
    pub fn get_services(&self) -> &HashMap<String, ServiceState> {
        &self.services
    }

    /// Get node info
    #[must_use]
    pub fn get_node(&self, node_id: NodeId) -> Option<&NodeInfo> {
        self.nodes.get(&node_id)
    }

    /// Get recent scale events
    #[must_use]
    pub fn get_scale_events(&self, limit: usize) -> &[ScaleEvent] {
        let start = self.scale_events.len().saturating_sub(limit);
        &self.scale_events[start..]
    }
}

/// Type alias for the configured Raft instance
pub type ZLayerRaft = Raft<TypeConfig>;

/// The state machine type, parameterized with our `ClusterState` and apply fn
pub type SchedulerStateMachine =
    MemStateMachine<TypeConfig, ClusterState, fn(&mut ClusterState, &Request) -> Response>;

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
// Raft Coordinator
// =============================================================================

/// High-level coordinator for Raft consensus
///
/// Wraps `zlayer_consensus::ConsensusNode` and provides a simpler API for:
/// - Bootstrapping a new cluster
/// - Joining an existing cluster
/// - Proposing state changes
/// - Reading cluster state
pub struct RaftCoordinator {
    /// `ConsensusNode` from zlayer-consensus
    node: ConsensusNode<TypeConfig>,
    /// Shared state machine data (for reading cluster state)
    sm_data: Arc<RwLock<SmData<TypeConfig, ClusterState>>>,
    /// Configuration (retained for callers that need `raft_port`, etc.)
    #[allow(dead_code)]
    config: RaftConfig,
}

impl RaftCoordinator {
    /// Create a new Raft coordinator without auth.
    pub async fn new(config: RaftConfig) -> Result<Self> {
        Self::with_auth(config, None).await
    }

    /// Create a new Raft coordinator with an optional bearer token for RPC auth.
    ///
    /// When `auth_token` is `Some`, the HTTP client will attach it to every
    /// outgoing Raft RPC as an `Authorization: Bearer <token>` header.
    pub async fn with_auth(config: RaftConfig, auth_token: Option<String>) -> Result<Self> {
        let consensus_config = ConsensusConfig {
            cluster_name: "zlayer".to_string(),
            heartbeat_interval_ms: config.heartbeat_interval_ms,
            election_timeout_min_ms: config.election_timeout_ms.0,
            election_timeout_max_ms: config.election_timeout_ms.1,
            rpc_timeout: Duration::from_millis(config.rpc_timeout_ms),
            ..ConsensusConfig::default()
        };

        // Create storage using zlayer-consensus's MemLogStore + MemStateMachine
        let log_store = MemLogStore::<TypeConfig>::new();
        let state_machine = MemStateMachine::<TypeConfig, ClusterState, _>::new(
            ClusterState::apply as fn(&mut ClusterState, &Request) -> Response,
        );
        let sm_data = state_machine.data();

        // Create network using zlayer-consensus's HttpNetwork (with optional auth)
        let network = HttpNetwork::<TypeConfig>::with_timeouts_and_auth(
            Duration::from_millis(config.rpc_timeout_ms),
            Duration::from_secs(60),
            auth_token,
        );

        // Build the consensus node
        let node = ConsensusNodeBuilder::new(config.node_id, config.address.clone())
            .with_config(consensus_config)
            .build_with(log_store, state_machine, network)
            .await
            .map_err(|e| SchedulerError::Raft(e.to_string()))?;

        info!(node_id = config.node_id, "Created Raft coordinator");

        Ok(Self {
            node,
            sm_data,
            config,
        })
    }

    /// Bootstrap a new single-node cluster
    ///
    /// This should only be called once when creating a new cluster.
    pub async fn bootstrap(&self) -> Result<()> {
        self.node
            .bootstrap()
            .await
            .map_err(|e| SchedulerError::Raft(format!("Bootstrap failed: {e}")))?;
        Ok(())
    }

    /// Check if this node is the current leader
    #[must_use]
    pub fn is_leader(&self) -> bool {
        self.node.is_leader()
    }

    /// Get the current leader's node ID
    #[must_use]
    pub fn leader_id(&self) -> Option<NodeId> {
        self.node.leader_id()
    }

    /// Propose a state change (must be leader)
    pub async fn propose(&self, request: Request) -> Result<Response> {
        self.node
            .propose(request)
            .await
            .map_err(|e| SchedulerError::Raft(format!("Failed to propose: {e}")))
    }

    /// Read current cluster state
    pub async fn read_state(&self) -> ClusterState {
        let sm = self.sm_data.read().await;
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
    #[must_use]
    pub fn metrics(&self) -> openraft::RaftMetrics<NodeId, BasicNode> {
        self.node.metrics()
    }

    /// Add a new member (voter) to the Raft cluster.
    ///
    /// The caller must be the current leader. This first adds the node as a
    /// **learner** (non-voting) so it can catch up on the log, then promotes
    /// it to a voting member via a membership change.
    pub async fn add_member(&self, params: AddMemberParams) -> Result<()> {
        // Use ConsensusNode's add_voter (learner + promote in one call)
        self.node
            .add_voter(params.node_id, params.addr.clone())
            .await
            .map_err(|e| {
                SchedulerError::Raft(format!("Failed to add member {}: {}", params.node_id, e))
            })?;

        // Register the node in the state machine so cluster state knows about it
        self.propose(Request::RegisterNode {
            node_id: params.node_id,
            address: params.addr.clone(),
            wg_public_key: params.wg_public_key,
            overlay_ip: params.overlay_ip,
            overlay_port: params.overlay_port,
            advertise_addr: params.advertise_addr,
            api_port: params.api_port,
            cpu_total: params.cpu_total,
            memory_total: params.memory_total,
            disk_total: params.disk_total,
            gpus: params.gpus,
        })
        .await?;

        info!(
            node_id = params.node_id,
            addr = %params.addr,
            "Added member to Raft cluster as voter"
        );

        Ok(())
    }

    /// Get this node's Raft node ID.
    #[must_use]
    pub fn node_id(&self) -> NodeId {
        self.config.node_id
    }

    /// Shutdown the Raft node
    pub async fn shutdown(&self) -> Result<()> {
        self.node
            .shutdown()
            .await
            .map_err(|e| SchedulerError::Raft(format!("Shutdown failed: {e}")))?;
        Ok(())
    }

    /// Get a reference to the underlying Raft instance.
    ///
    /// This is used by the Raft RPC service to forward RPCs to the Raft node.
    #[must_use]
    pub fn raft_handle(&self) -> &ZLayerRaft {
        self.node.raft()
    }

    /// Get a clone of the underlying Raft instance (cheap, Arc-based).
    ///
    /// Used to construct the `raft_service_router()` from `zlayer-consensus`.
    #[must_use]
    pub fn raft_clone(&self) -> ZLayerRaft {
        self.node.raft_clone()
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
            wg_public_key: String::new(),
            overlay_ip: String::new(),
            overlay_port: 0,
            advertise_addr: String::new(),
            api_port: 0,
            cpu_total: 8.0,
            memory_total: 16_000_000_000,
            disk_total: 500_000_000_000,
            gpus: vec![],
        });

        let node = state.get_node(1).unwrap();
        assert_eq!(node.address, "192.168.1.1:8000");
        assert_eq!(node.cpu_total, 8.0);
        assert_eq!(node.memory_total, 16_000_000_000);
        assert_eq!(node.status, "ready");
    }
}
