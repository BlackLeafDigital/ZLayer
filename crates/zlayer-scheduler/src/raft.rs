//! `OpenRaft` distributed coordination for `ZLayer` scheduler
//!
//! Provides consensus-based service state management across nodes.
//! Uses `zlayer-consensus` for storage, network, and Raft RPC plumbing.

use std::collections::{BTreeSet, HashMap};
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use openraft::{BasicNode, Raft};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::info;
use utoipa::ToSchema;
use zlayer_consensus::network::http_client::HttpNetwork;
#[cfg(not(feature = "persistent"))]
use zlayer_consensus::storage::mem_store::{MemLogStore, MemStateMachine, SmData};
#[cfg(feature = "persistent")]
use zlayer_consensus::storage::zql_store::{ZqlLogStore, ZqlSmCache, ZqlStateMachine};
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
        #[serde(default)]
        mode: String,
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

/// The role assigned to a cluster member in the Raft consensus layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemberRole {
    /// Full voting member (contributes to quorum).
    Voter,
    /// Non-voting learner (receives replication but doesn't vote).
    Learner,
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
/// This is the `S` generic parameter to `ZqlStateMachine<TypeConfig, S, F>`.
/// It only contains application-level fields -- the Raft bookkeeping
/// (`last_applied_log`, `last_membership`) is managed by `ZqlSmCache` inside the
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
    /// Join mode: "full" (eligible to be voter) or "replicate" (always learner)
    #[serde(default = "default_node_mode")]
    pub mode: String,
}

fn default_node_status() -> String {
    "ready".to_string()
}

fn default_node_mode() -> String {
    "full".to_string()
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
    /// Join mode: "full" or "replicate"
    pub mode: String,
}

impl ClusterState {
    /// Create a new empty cluster state
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a request to the state machine
    ///
    /// # Panics
    ///
    /// Panics if the system clock is before the Unix epoch (only relevant for
    /// `RegisterNode` requests).
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
            } => self.apply_record_scale_event(
                service_name,
                *from_replicas,
                *to_replicas,
                reason,
                *timestamp,
            ),
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
                mode,
            } => self.apply_register_node(
                *node_id,
                address,
                wg_public_key,
                overlay_ip,
                *overlay_port,
                advertise_addr,
                *api_port,
                *cpu_total,
                *memory_total,
                *disk_total,
                gpus,
                mode,
            ),
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
                    node.status.clone_from(status);
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
                    svc.assigned_nodes.clone_from(node_ids);
                    Response::Success { data: None }
                } else {
                    Response::Error {
                        message: format!("Service not found: {service_name}"),
                    }
                }
            }
        }
    }

    /// Apply a `RecordScaleEvent` request.
    fn apply_record_scale_event(
        &mut self,
        service_name: &str,
        from_replicas: u32,
        to_replicas: u32,
        reason: &str,
        timestamp: u64,
    ) -> Response {
        let event = ScaleEvent {
            service_name: service_name.to_owned(),
            from_replicas,
            to_replicas,
            reason: reason.to_owned(),
            timestamp,
        };

        // Keep last 100 events
        self.scale_events.push(event);
        if self.scale_events.len() > 100 {
            self.scale_events.remove(0);
        }

        // Update last scale time on service
        if let Some(svc) = self.services.get_mut(service_name) {
            svc.last_scale_time = Some(timestamp);
            svc.current_replicas = to_replicas;
        }

        Response::Success { data: None }
    }

    /// Apply a `RegisterNode` request.
    #[allow(clippy::too_many_arguments)]
    fn apply_register_node(
        &mut self,
        node_id: NodeId,
        address: &str,
        wg_public_key: &str,
        overlay_ip: &str,
        overlay_port: u16,
        advertise_addr: &str,
        api_port: u16,
        cpu_total: f64,
        memory_total: u64,
        disk_total: u64,
        gpus: &[GpuInfoSummary],
        mode: &str,
    ) -> Response {
        let now = u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap_or(u64::MAX);

        self.nodes.insert(
            node_id,
            NodeInfo {
                node_id,
                address: address.to_owned(),
                registered_at: now,
                last_heartbeat: now,
                gpus: gpus.to_vec(),
                wg_public_key: wg_public_key.to_owned(),
                overlay_ip: overlay_ip.to_owned(),
                overlay_port,
                advertise_addr: advertise_addr.to_owned(),
                api_port,
                cpu_total,
                memory_total,
                disk_total,
                cpu_used: 0.0,
                memory_used: 0,
                disk_used: 0,
                status: "ready".to_string(),
                mode: mode.to_owned(),
            },
        );

        Response::Success { data: None }
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
#[cfg(feature = "persistent")]
pub type SchedulerStateMachine =
    ZqlStateMachine<TypeConfig, ClusterState, fn(&mut ClusterState, &Request) -> Response>;
#[cfg(not(feature = "persistent"))]
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
    /// Path to the data directory for persistent Raft storage
    pub data_dir: PathBuf,
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
            data_dir: zlayer_paths::ZLayerDirs::system_default().raft(),
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
    #[cfg(feature = "persistent")]
    sm_data: Arc<RwLock<ZqlSmCache<TypeConfig, ClusterState>>>,
    #[cfg(not(feature = "persistent"))]
    sm_data: Arc<RwLock<SmData<TypeConfig, ClusterState>>>,
    /// Configuration (retained for callers that need `raft_port`, etc.)
    #[allow(dead_code)]
    config: RaftConfig,
}

impl RaftCoordinator {
    /// Create a new Raft coordinator without auth.
    ///
    /// # Errors
    ///
    /// Returns an error if the data directory cannot be created, or if the
    /// log store, state machine, or consensus node fails to initialize.
    pub async fn new(config: RaftConfig) -> Result<Self> {
        Self::with_auth(config, None).await
    }

    /// Create a new Raft coordinator with an optional bearer token for RPC auth.
    ///
    /// When `auth_token` is `Some`, the HTTP client will attach it to every
    /// outgoing Raft RPC as an `Authorization: Bearer <token>` header.
    ///
    /// # Errors
    ///
    /// Returns an error if the data directory cannot be created, or if the
    /// log store, state machine, or consensus node fails to initialize.
    pub async fn with_auth(config: RaftConfig, auth_token: Option<String>) -> Result<Self> {
        let consensus_config = ConsensusConfig {
            cluster_name: "zlayer".to_string(),
            heartbeat_interval_ms: config.heartbeat_interval_ms,
            election_timeout_min_ms: config.election_timeout_ms.0,
            election_timeout_max_ms: config.election_timeout_ms.1,
            rpc_timeout: Duration::from_millis(config.rpc_timeout_ms),
            ..ConsensusConfig::default()
        };

        // Ensure data directory exists
        std::fs::create_dir_all(&config.data_dir)
            .map_err(|e| SchedulerError::Raft(format!("Failed to create raft data dir: {e}")))?;

        // Create storage (persistent ZQL or in-memory depending on feature)
        #[cfg(feature = "persistent")]
        let (log_store, state_machine, sm_data) = {
            let log_path = config.data_dir.join("raft-log");
            let sm_path = config.data_dir.join("raft-sm");
            let log_store = ZqlLogStore::<TypeConfig>::new(&log_path)
                .map_err(|e| SchedulerError::Raft(format!("Failed to open raft log store: {e}")))?;
            let state_machine = ZqlStateMachine::<TypeConfig, ClusterState, _>::new(
                &sm_path,
                ClusterState::apply as fn(&mut ClusterState, &Request) -> Response,
            )
            .map_err(|e| SchedulerError::Raft(format!("Failed to open raft state machine: {e}")))?;
            let sm_data = state_machine.state();
            (log_store, state_machine, sm_data)
        };
        #[cfg(not(feature = "persistent"))]
        let (log_store, state_machine, sm_data) = {
            let log_store = MemLogStore::<TypeConfig>::default();
            let state_machine = MemStateMachine::<TypeConfig, ClusterState, _>::new(
                ClusterState::apply as fn(&mut ClusterState, &Request) -> Response,
            );
            let sm_data = state_machine.data();
            (log_store, state_machine, sm_data)
        };

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
    ///
    /// # Errors
    ///
    /// Returns an error if the Raft bootstrap operation fails (e.g., cluster
    /// already bootstrapped).
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
    ///
    /// # Errors
    ///
    /// Returns an error if this node is not the leader or if the Raft proposal
    /// fails to reach consensus.
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
    ///
    /// # Errors
    ///
    /// Returns an error if this node is not the leader or if the Raft proposal
    /// fails to reach consensus.
    pub async fn update_service(&self, name: String, state: ServiceState) -> Result<()> {
        self.propose(Request::UpdateServiceState {
            service_name: name,
            state,
        })
        .await?;
        Ok(())
    }

    /// Record a scale event
    ///
    /// # Errors
    ///
    /// Returns an error if this node is not the leader or if the Raft proposal
    /// fails to reach consensus.
    ///
    /// # Panics
    ///
    /// Panics if the system clock is before the Unix epoch.
    pub async fn record_scale_event(
        &self,
        service_name: String,
        from_replicas: u32,
        to_replicas: u32,
        reason: String,
    ) -> Result<()> {
        let timestamp = u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap_or(u64::MAX);

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

    /// Add a new member to the Raft cluster.
    ///
    /// The caller must be the current leader. The node is always added as a
    /// **learner** first so it can catch up on the log. If `params.mode` is
    /// `"full"`, `rebalance_voters()` is called afterwards which may promote
    /// the node (or others) to voter status. If `params.mode` is `"replicate"`,
    /// the node stays as a learner permanently.
    ///
    /// Returns the [`MemberRole`] that was assigned to the new member.
    ///
    /// # Errors
    ///
    /// Returns an error if this node is not the leader, if the learner
    /// addition fails, or if the `RegisterNode` proposal fails to reach
    /// consensus.
    pub async fn add_member(&self, params: AddMemberParams) -> Result<MemberRole> {
        // Step 1: Always add as a learner first (blocking so it catches up)
        self.node
            .add_learner(params.node_id, params.addr.clone(), true)
            .await
            .map_err(|e| {
                SchedulerError::Raft(format!("Failed to add learner {}: {}", params.node_id, e))
            })?;

        // Step 2: Register the node in the state machine
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
            mode: params.mode.clone(),
        })
        .await?;

        // Step 3: Determine role based on mode
        if params.mode == "replicate" {
            info!(
                node_id = params.node_id,
                addr = %params.addr,
                "Added member to Raft cluster as learner (replicate mode)"
            );
            return Ok(MemberRole::Learner);
        }

        // "full" mode: rebalance voters, then check if this node became a voter
        self.rebalance_voters().await?;

        let role = if self.node.voter_ids().contains(&params.node_id) {
            MemberRole::Voter
        } else {
            MemberRole::Learner
        };

        info!(
            node_id = params.node_id,
            addr = %params.addr,
            role = ?role,
            "Added member to Raft cluster"
        );

        Ok(role)
    }

    /// Rebalance the voter set to maintain an optimal odd number of voters.
    ///
    /// Reads the current cluster state to find all "full" mode nodes (eligible voters),
    /// then computes the target voter count and adjusts membership accordingly.
    /// Nodes with mode "replicate" are never promoted to voter.
    ///
    /// # Errors
    /// Returns an error if the membership change fails.
    pub async fn rebalance_voters(&self) -> Result<()> {
        // Read cluster state to get node modes
        let cluster_state = self.read_state().await;

        // Current Raft membership
        let current_voters = self.node.voter_ids();
        let current_learners = self.node.learner_ids();

        // Eligible nodes: registered, mode == "full", and present in Raft membership
        let all_members: BTreeSet<NodeId> = current_voters
            .iter()
            .chain(current_learners.iter())
            .copied()
            .collect();

        let eligible: Vec<NodeId> = all_members
            .iter()
            .filter(|id| {
                cluster_state
                    .nodes
                    .get(id)
                    .is_some_and(|n| n.mode == "full" && n.status != "dead")
            })
            .copied()
            .collect();

        let target = target_voters(eligible.len());

        // Current voters that are still eligible
        let mut surviving_voters: Vec<NodeId> = current_voters
            .iter()
            .filter(|id| eligible.contains(id))
            .copied()
            .collect();
        surviving_voters.sort_unstable();

        if surviving_voters.len() == target {
            // Already balanced
            return Ok(());
        }

        let new_voter_ids: BTreeSet<NodeId> = if surviving_voters.len() < target {
            // Need more voters — promote eligible learners (prefer lower IDs)
            let mut voters: BTreeSet<NodeId> = surviving_voters.iter().copied().collect();
            let mut candidates: Vec<NodeId> = eligible
                .iter()
                .filter(|id| !voters.contains(id))
                .copied()
                .collect();
            candidates.sort_unstable();

            for id in candidates {
                if voters.len() >= target {
                    break;
                }
                voters.insert(id);
            }
            voters
        } else {
            // Too many voters — demote excess (keep the leader + lowest IDs)
            let leader_id = self.node.leader_id();

            // Partition into leader and non-leader, keeping order
            let mut keep: Vec<NodeId> = Vec::with_capacity(target);

            // Always keep the leader first if it's among survivors
            if let Some(lid) = leader_id {
                if surviving_voters.contains(&lid) {
                    keep.push(lid);
                }
            }

            // Fill remaining slots with lowest IDs (excluding leader already added)
            for &id in &surviving_voters {
                if keep.len() >= target {
                    break;
                }
                if !keep.contains(&id) {
                    keep.push(id);
                }
            }

            keep.into_iter().collect()
        };

        // Apply the membership change (retain=true keeps demoted nodes as learners)
        self.node
            .change_membership(new_voter_ids, true)
            .await
            .map_err(|e| SchedulerError::Raft(format!("Failed to rebalance voters: {e}")))?;

        info!("Rebalanced voter set (target={target})");
        Ok(())
    }

    /// Remove a member from the Raft cluster entirely.
    ///
    /// This removes the node from both the Raft membership and the application
    /// state machine. After removal, the node will no longer receive log replication.
    ///
    /// # Errors
    /// Returns an error if this node is not the leader or if the removal fails.
    pub async fn remove_member(&self, node_id: NodeId) -> Result<()> {
        // Get current voters and remove this node
        let mut voter_ids = self.node.voter_ids();
        voter_ids.remove(&node_id);

        // Change membership with retain=false to fully remove the node
        self.node
            .change_membership(voter_ids, false)
            .await
            .map_err(|e| {
                SchedulerError::Raft(format!(
                    "Failed to remove member {node_id} from membership: {e}"
                ))
            })?;

        // Remove from the application state machine
        self.propose(Request::DeregisterNode { node_id }).await?;

        // Rebalance remaining voters
        self.rebalance_voters().await?;

        info!(node_id, "Removed member from Raft cluster");
        Ok(())
    }

    /// Get this node's Raft node ID.
    #[must_use]
    pub fn node_id(&self) -> NodeId {
        self.config.node_id
    }

    /// Shutdown the Raft node
    ///
    /// # Errors
    ///
    /// Returns an error if the Raft shutdown operation fails.
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

/// Compute the target number of voters for optimal fault tolerance.
///
/// Rules:
/// - Always an odd number (prevents tied votes)
/// - Minimum 1
/// - Maximum 7 (beyond 7 voters, consensus overhead outweighs benefits)
/// - Formula: min(7, largest odd number <= `eligible_count`)
#[must_use]
pub fn target_voters(eligible_count: usize) -> usize {
    if eligible_count == 0 {
        return 1;
    }
    let capped = eligible_count.min(7);
    if capped % 2 == 0 {
        capped - 1
    } else {
        capped
    }
}

/// Path to the force-leader recovery marker file.
#[must_use]
pub fn force_leader_marker_path(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("force_leader_recovery.json")
}

/// Save cluster state for force-leader recovery.
///
/// Writes a JSON file that the daemon checks on startup. When found, the daemon
/// wipes Raft storage and re-bootstraps as a single-node leader, then replays
/// the saved state.
///
/// # Errors
/// Returns an error if the file cannot be written.
pub fn save_force_leader_state(
    data_dir: &std::path::Path,
    state: &ClusterState,
) -> std::result::Result<(), std::io::Error> {
    let path = force_leader_marker_path(data_dir);
    let json = serde_json::to_string_pretty(state).map_err(std::io::Error::other)?;
    std::fs::write(path, json)
}

/// Check for and load a force-leader recovery marker.
///
/// Returns `Some(ClusterState)` if a recovery is pending.
/// Deletes the marker file after reading.
///
/// # Errors
/// Returns an error if the file exists but cannot be read or parsed.
pub fn load_and_clear_force_leader_state(
    data_dir: &std::path::Path,
) -> std::result::Result<Option<ClusterState>, std::io::Error> {
    let path = force_leader_marker_path(data_dir);
    if !path.exists() {
        return Ok(None);
    }
    let json = std::fs::read_to_string(&path)?;
    let state: ClusterState = serde_json::from_str(&json)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::remove_file(&path)?;
    Ok(Some(state))
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
            timestamp: 1_234_567_890,
        });

        let events = state.get_scale_events(10);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].to_replicas, 4);

        // Service should be updated
        let svc = state.get_service("api").unwrap();
        assert_eq!(svc.current_replicas, 4);
        assert_eq!(svc.last_scale_time, Some(1_234_567_890));
    }

    #[test]
    #[allow(clippy::float_cmp)]
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
            mode: "full".to_string(),
        });

        let node = state.get_node(1).unwrap();
        assert_eq!(node.address, "192.168.1.1:8000");
        assert_eq!(node.cpu_total, 8.0);
        assert_eq!(node.memory_total, 16_000_000_000);
        assert_eq!(node.status, "ready");
    }

    #[test]
    fn test_target_voters() {
        assert_eq!(target_voters(0), 1);
        assert_eq!(target_voters(1), 1);
        assert_eq!(target_voters(2), 1);
        assert_eq!(target_voters(3), 3);
        assert_eq!(target_voters(4), 3);
        assert_eq!(target_voters(5), 5);
        assert_eq!(target_voters(6), 5);
        assert_eq!(target_voters(7), 7);
        assert_eq!(target_voters(8), 7);
        assert_eq!(target_voters(100), 7);
    }

    #[test]
    fn test_force_leader_state_save_load() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = ClusterState::new();
        state.apply(&Request::UpdateServiceState {
            service_name: "test-svc".to_string(),
            state: ServiceState {
                current_replicas: 3,
                desired_replicas: 5,
                ..Default::default()
            },
        });
        state.apply(&Request::RegisterNode {
            node_id: 1,
            address: "10.0.0.1:9000".to_string(),
            wg_public_key: String::new(),
            overlay_ip: "10.200.0.1".to_string(),
            overlay_port: 51820,
            advertise_addr: "10.0.0.1".to_string(),
            api_port: 3669,
            cpu_total: 8.0,
            memory_total: 16_000_000_000,
            disk_total: 500_000_000_000,
            gpus: vec![],
            mode: "full".to_string(),
        });

        // Save
        save_force_leader_state(dir.path(), &state).unwrap();
        assert!(force_leader_marker_path(dir.path()).exists());

        // Load
        let loaded = load_and_clear_force_leader_state(dir.path()).unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.services.len(), 1);
        assert_eq!(loaded.nodes.len(), 1);
        assert_eq!(loaded.get_service("test-svc").unwrap().current_replicas, 3);

        // Marker should be deleted
        assert!(!force_leader_marker_path(dir.path()).exists());
    }

    #[test]
    fn test_force_leader_state_save_load_ipv6_overlay() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = ClusterState::new();
        state.apply(&Request::UpdateServiceState {
            service_name: "test-svc-v6".to_string(),
            state: ServiceState {
                current_replicas: 2,
                desired_replicas: 4,
                ..Default::default()
            },
        });
        state.apply(&Request::RegisterNode {
            node_id: 2,
            address: "10.0.0.2:9000".to_string(),
            wg_public_key: "wg-pub-key-v6".to_string(),
            overlay_ip: "fd00:200::1".to_string(),
            overlay_port: 51820,
            advertise_addr: "10.0.0.2".to_string(),
            api_port: 3669,
            cpu_total: 16.0,
            memory_total: 32_000_000_000,
            disk_total: 1_000_000_000_000,
            gpus: vec![],
            mode: "full".to_string(),
        });

        // Save
        save_force_leader_state(dir.path(), &state).unwrap();
        assert!(force_leader_marker_path(dir.path()).exists());

        // Load
        let loaded = load_and_clear_force_leader_state(dir.path()).unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.services.len(), 1);
        assert_eq!(loaded.nodes.len(), 1);
        let node = loaded.get_node(2).unwrap();
        assert_eq!(node.overlay_ip, "fd00:200::1");
        assert_eq!(node.wg_public_key, "wg-pub-key-v6");
        assert_eq!(
            loaded.get_service("test-svc-v6").unwrap().current_replicas,
            2
        );

        // Marker should be deleted
        assert!(!force_leader_marker_path(dir.path()).exists());
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn test_node_registration_with_ipv6_overlay() {
        let mut state = ClusterState::new();

        state.apply(&Request::RegisterNode {
            node_id: 3,
            address: "192.168.1.3:8000".to_string(),
            wg_public_key: "wg-key-node3".to_string(),
            overlay_ip: "fd00:200::3".to_string(),
            overlay_port: 51820,
            advertise_addr: "192.168.1.3".to_string(),
            api_port: 3669,
            cpu_total: 4.0,
            memory_total: 8_000_000_000,
            disk_total: 250_000_000_000,
            gpus: vec![],
            mode: "full".to_string(),
        });

        let node = state.get_node(3).unwrap();
        assert_eq!(node.address, "192.168.1.3:8000");
        assert_eq!(node.overlay_ip, "fd00:200::3");
        assert_eq!(node.overlay_port, 51820);
        assert_eq!(node.wg_public_key, "wg-key-node3");
        assert_eq!(node.cpu_total, 4.0);
        assert_eq!(node.status, "ready");
    }

    #[test]
    fn test_force_leader_no_marker() {
        let dir = tempfile::tempdir().unwrap();
        let result = load_and_clear_force_leader_state(dir.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_node_info_mode_default() {
        let json =
            r#"{"node_id":1,"address":"10.0.0.1:9000","registered_at":0,"last_heartbeat":0}"#;
        let info: NodeInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.mode, "full");
        assert_eq!(info.status, "ready");
    }

    #[test]
    fn test_member_role_serialization() {
        let voter = MemberRole::Voter;
        let learner = MemberRole::Learner;
        assert_eq!(serde_json::to_string(&voter).unwrap(), "\"Voter\"");
        assert_eq!(serde_json::to_string(&learner).unwrap(), "\"Learner\"");
    }
}
