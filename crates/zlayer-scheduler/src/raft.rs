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
use tracing::{info, warn};
use utoipa::ToSchema;
use zlayer_consensus::network::http_client::HttpNetwork;
#[cfg(not(feature = "persistent"))]
use zlayer_consensus::storage::mem_store::{MemLogStore, MemStateMachine, SmData};
#[cfg(feature = "persistent")]
use zlayer_consensus::storage::redb_store::{RedbLogStore, RedbSmCache, RedbStateMachine};
use zlayer_consensus::{ConsensusConfig, ConsensusNode, ConsensusNodeBuilder};
use zlayer_secrets::cluster_dek::ClusterDek;
use zlayer_secrets::sealed::{RecipientPrivateKey, RecipientPublicKey};
use zlayer_secrets::{NodeSideEffects, SecretsError, SecretsState};
use zlayer_types::api::internal::SecretsRaftOp;
use zlayer_types::storage::{NodeIdentity, ReplicatedSecret};

use crate::error::{Result, SchedulerError};
#[cfg(test)]
use zlayer_paths::ZLayerDirs;

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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        os: Option<zlayer_spec::OsKind>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        arch: Option<zlayer_spec::ArchKind>,
        /// Per-node slice of the cluster CIDR assigned by the leader (e.g. "10.200.42.0/28").
        /// Empty string for pre-slice-aware registrations — treated as "no slice".
        #[serde(default)]
        slice_cidr: String,
    },
    /// Update node heartbeat with current resource usage
    UpdateNodeHeartbeat {
        node_id: NodeId,
        timestamp: u64,
        cpu_used: f64,
        memory_used: u64,
        disk_used: u64,
        #[serde(default)]
        gpu_utilization: Vec<GpuUtilizationReport>,
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
    /// Cluster-replicated secrets operation.
    ///
    /// Wraps a [`SecretsRaftOp`] (defined in `zlayer-types`) so the secrets
    /// state machine in `zlayer-secrets::raft_sm::SecretsState` can be
    /// driven through the same Raft log as the scheduler's own ops.
    ///
    /// This variant is intentionally **appended last** so its postcard2
    /// discriminant does not shift any of the pre-existing variants — older
    /// log entries stay decodable bit-for-bit.
    Secrets(SecretsRaftOp),
    /// Update a node's membership mode (`full` or `replicate`).
    UpdateNodeMode { node_id: NodeId, mode: String },
    /// Grant a new worker lease — emitted when a worker successfully Registers.
    GrantWorkerLease {
        node_id: NodeId,
        holder: String,   // worker's mTLS cn or token-cn
        acquired_ns: u64, // unix nanos
        renewed_ns: u64,  // unix nanos (== acquired_ns on first grant)
        ttl_secs: u32,
    },
    /// Renew an existing worker lease — emitted on every successful `ReportStatus`
    /// ack tick. Only updates `renewed_ns` and `ttl_secs`; the lease stays alive.
    RenewWorkerLease {
        node_id: NodeId,
        renewed_ns: u64,
        ttl_secs: u32,
    },
    /// Expire a worker lease — emitted by the leader's expiry sweep tick when
    /// `renewed_ns + ttl_secs + grace_secs < now`. Idempotent: removes the
    /// lease if present, no-op otherwise.
    ExpireWorkerLease { node_id: NodeId },
    /// Revoke a worker lease — admin action (e.g. `worker-evict`). Same effect
    /// as `ExpireWorkerLease` but distinct for audit/log readability.
    RevokeWorkerLease { node_id: NodeId },
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

/// FSM-internal record of a worker lease. Mirrors `zlayer_types::cluster::WorkerLease`
/// but uses u64 unix-nanos so postcard serialization stays stable across versions.
///
/// Converted to/from `zlayer_types::cluster::WorkerLease` at the read boundary
/// (queries that return leases to callers do the `SystemTime` conversion there).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerLeaseRecord {
    pub node_id: NodeId,
    pub holder: String,
    pub acquired_ns: u64,
    pub renewed_ns: u64,
    pub ttl_secs: u32,
    /// Monotonic revision, incremented on every renew. Lets the leader-side
    /// dispatcher push updates that include the latest revision the worker
    /// observed so reconnects can fast-forward.
    pub revision: u64,
}

impl WorkerLeaseRecord {
    /// Returns true if `now_ns >= renewed_ns + (ttl_secs + grace_secs) * 1e9`.
    #[must_use]
    pub fn is_expired(&self, now_ns: u64, grace_secs: u32) -> bool {
        let deadline_ns = self
            .renewed_ns
            .saturating_add(u64::from(self.ttl_secs).saturating_mul(1_000_000_000))
            .saturating_add(u64::from(grace_secs).saturating_mul(1_000_000_000));
        now_ns >= deadline_ns
    }
}

/// Cluster state (the Raft state machine application state).
///
/// This is the `S` generic parameter to the storage state machine types.
/// It only contains application-level fields -- the Raft bookkeeping
/// (`last_applied_log`, `last_membership`) is managed by the consensus crate.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClusterState {
    /// Service states
    pub services: HashMap<String, ServiceState>,
    /// Node registry
    pub nodes: HashMap<NodeId, NodeInfo>,
    /// Recent scale events (ring buffer, keep last 100)
    pub scale_events: Vec<ScaleEvent>,
    /// Cluster-replicated secrets state.
    ///
    /// `#[serde(default)]` so snapshots written before this field existed
    /// (any node still on the pre-secrets `ClusterState` shape) restore
    /// cleanly with an empty [`SecretsState`].
    #[serde(default)]
    pub secrets: SecretsState,
    /// Worker-tier leases (Nomad-style). Keyed by `node_id` of the worker.
    /// Only populated when the cluster is in `worker-tier` mode; empty in
    /// single-node / static / pure-raft modes. `#[serde(default)]` keeps old
    /// snapshots loading cleanly.
    #[serde(default)]
    pub worker_leases: HashMap<NodeId, WorkerLeaseRecord>,
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
    /// Current GPU utilization snapshots
    #[serde(default)]
    pub gpu_utilization: Vec<GpuUtilizationReport>,
    /// Node status: "ready", "draining", or "dead"
    #[serde(default = "default_node_status")]
    pub status: String,
    /// Join mode: "full" (eligible to be voter) or "replicate" (always learner)
    #[serde(default = "default_node_mode")]
    pub mode: String,
    /// Operating system of the agent running this node. `None` = agent
    /// predates this field (legacy registration) — treated as "unknown"
    /// by the scheduler's platform filter, which will skip platform-matching
    /// entirely in that case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<zlayer_spec::OsKind>,
    /// CPU architecture of the agent. Same legacy semantics as `os`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<zlayer_spec::ArchKind>,
    /// Per-node slice of the cluster CIDR assigned by the leader (e.g. "10.200.42.0/28").
    /// Empty string for pre-slice-aware registrations — treated as "no slice".
    #[serde(default)]
    pub slice_cidr: String,
}

fn default_node_status() -> String {
    "ready".to_string()
}

fn default_node_mode() -> String {
    "full".to_string()
}

/// Summary of a GPU on a node, stored in Raft cluster state.
///
/// Lifted into [`zlayer_types::api::nodes::GpuInfoSummary`]; re-exported
/// here for source compatibility with existing call sites.
pub use zlayer_types::api::nodes::GpuInfoSummary;

/// Per-GPU utilization snapshot reported in node heartbeats
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GpuUtilizationReport {
    /// GPU index on this node
    pub index: u32,
    /// GPU compute utilization percentage (0-100)
    pub utilization_percent: f32,
    /// GPU memory currently used in MB
    pub memory_used_mb: u64,
    /// GPU total memory in MB
    pub memory_total_mb: u64,
    /// GPU temperature in Celsius
    pub temperature_c: Option<u32>,
    /// GPU power draw in Watts
    pub power_draw_w: Option<f32>,
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
    /// Operating system of the joining agent. `None` = legacy client that did
    /// not report platform info (treated as "unknown" downstream).
    pub os: Option<zlayer_spec::OsKind>,
    /// CPU architecture of the joining agent. Same legacy semantics as `os`.
    pub arch: Option<zlayer_spec::ArchKind>,
    /// Per-node slice of the cluster CIDR assigned by the leader (e.g. "10.200.42.0/28").
    /// Empty string for pre-slice-aware registrations — treated as "no slice".
    pub slice_cidr: String,
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
    #[allow(clippy::too_many_lines)]
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
                os,
                arch,
                slice_cidr,
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
                *os,
                *arch,
                slice_cidr,
            ),
            Request::UpdateNodeHeartbeat {
                node_id,
                timestamp,
                cpu_used,
                memory_used,
                disk_used,
                gpu_utilization,
            } => {
                if let Some(node) = self.nodes.get_mut(node_id) {
                    node.last_heartbeat = *timestamp;
                    node.cpu_used = *cpu_used;
                    node.memory_used = *memory_used;
                    node.disk_used = *disk_used;
                    node.gpu_utilization.clone_from(gpu_utilization);
                }
                Response::Success { data: None }
            }
            Request::UpdateNodeStatus { node_id, status } => {
                if let Some(node) = self.nodes.get_mut(node_id) {
                    node.status.clone_from(status);
                }
                Response::Success { data: None }
            }
            Request::UpdateNodeMode { node_id, mode } => {
                if mode != "full" && mode != "replicate" {
                    return Response::Error {
                        message: format!(
                            "Invalid node mode: {mode} (expected \"full\" or \"replicate\")"
                        ),
                    };
                }
                if let Some(node) = self.nodes.get_mut(node_id) {
                    node.mode.clone_from(mode);
                    Response::Success {
                        data: Some(format!("node {node_id} mode = {mode}")),
                    }
                } else {
                    Response::Error {
                        message: format!("Node not found: {node_id}"),
                    }
                }
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
            Request::Secrets(op) => self.apply_secrets(op),
            Request::GrantWorkerLease {
                node_id,
                holder,
                acquired_ns,
                renewed_ns,
                ttl_secs,
            } => {
                // Re-grant (re-register) is OK: overwrite existing record with
                // fresh acquired_ns. Revision restarts at 1.
                let record = WorkerLeaseRecord {
                    node_id: *node_id,
                    holder: holder.clone(),
                    acquired_ns: *acquired_ns,
                    renewed_ns: *renewed_ns,
                    ttl_secs: *ttl_secs,
                    revision: 1,
                };
                self.worker_leases.insert(*node_id, record);
                Response::Success { data: None }
            }
            Request::RenewWorkerLease {
                node_id,
                renewed_ns,
                ttl_secs,
            } => {
                if let Some(lease) = self.worker_leases.get_mut(node_id) {
                    // Reject monotonic time-travel: never advance renewed_ns
                    // backwards. Stale acks (out-of-order replay) are no-ops.
                    if *renewed_ns > lease.renewed_ns {
                        lease.renewed_ns = *renewed_ns;
                        lease.ttl_secs = *ttl_secs;
                        lease.revision = lease.revision.saturating_add(1);
                    }
                    Response::Success { data: None }
                } else {
                    // Renew without a prior Grant — happens after ExpireWorkerLease
                    // races with an in-flight renewal. Caller should re-Register.
                    Response::Error {
                        message: format!("RenewWorkerLease: no lease for node {node_id}"),
                    }
                }
            }
            Request::ExpireWorkerLease { node_id } | Request::RevokeWorkerLease { node_id } => {
                // Idempotent removal. Both variants share apply-logic; the
                // distinction is only for log audit / metrics.
                self.worker_leases.remove(node_id);
                Response::Success { data: None }
            }
        }
    }

    /// Apply a [`SecretsRaftOp`] to the inner [`SecretsState`].
    ///
    /// Extracted from [`Self::apply`] both to keep that match arm small and
    /// to give clippy a saner per-function line budget.
    fn apply_secrets(&mut self, op: &SecretsRaftOp) -> Response {
        match self.secrets.apply(op.clone()) {
            Ok(()) => Response::Success { data: None },
            Err(e) => {
                // Determinism guard: every replica must reach the same
                // outcome on the same op. `SecretsState::apply` only
                // returns an error for "impossible" inputs (revoke
                // unknown node, delete unknown secret) — log so an
                // operator can audit, but treat the apply as a no-op
                // on the SM side and surface the message back to the
                // caller via `Response::Error`. Followers will produce
                // the same `Response::Error` for the same log index,
                // keeping the SM convergent.
                warn!(
                    op = ?op,
                    error = %e,
                    "SecretsRaftOp apply returned error",
                );
                Response::Error {
                    message: format!("Secrets apply failed: {e}"),
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
        os: Option<zlayer_spec::OsKind>,
        arch: Option<zlayer_spec::ArchKind>,
        slice_cidr: &str,
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
                gpu_utilization: Vec::new(),
                status: "ready".to_string(),
                mode: mode.to_owned(),
                os,
                arch,
                slice_cidr: slice_cidr.to_owned(),
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
    RedbStateMachine<TypeConfig, ClusterState, fn(&mut ClusterState, &Request) -> Response>;
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

/// Whether the coordinator just created fresh raft state or loaded
/// existing state from disk.
///
/// Surfaces the "is this a restart?" signal to callers so they can
/// skip `bootstrap()` on resume — avoiding openraft's internal
/// `error!("Can not initialize ...")` that would otherwise fire when
/// bootstrap is invoked on already-initialised state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapState {
    /// No prior raft state on disk; cluster will be bootstrapped if
    /// this node is the leader.
    Fresh,
    /// Prior raft state was loaded from disk; caller should NOT call
    /// `bootstrap()`.
    Resuming,
}

/// Result of constructing a coordinator: the coordinator itself plus
/// the bootstrap state observed at construction time.
pub struct CoordinatorInit {
    /// The freshly constructed coordinator.
    pub coordinator: RaftCoordinator,
    /// Whether on-disk raft state was present at construction time.
    pub bootstrap_state: BootstrapState,
}

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
    sm_data: Arc<RwLock<RedbSmCache<TypeConfig, ClusterState>>>,
    #[cfg(not(feature = "persistent"))]
    sm_data: Arc<RwLock<SmData<TypeConfig, ClusterState>>>,
    /// Configuration (retained for callers that need `raft_port`, etc.)
    #[allow(dead_code)]
    config: RaftConfig,
    /// Local node X25519 private key for unwrapping cluster DEKs.
    ///
    /// Required by leader-side secrets orchestration helpers
    /// (`propose_register_node_and_rotate`, `propose_rotate_dek`). When
    /// unset, those helpers return [`SecretsError::Provider`] explaining
    /// that the node was constructed without secrets capability.
    ///
    /// Wrapped in `Arc` so [`RaftCoordinator`] stays cheap to share, and
    /// because [`RecipientPrivateKey`] is not [`Clone`].
    node_priv: Option<Arc<RecipientPrivateKey>>,
    /// Local node UUID — the key under which `WrappedDek::wraps` stores
    /// this node's wrap. Set alongside `node_priv`.
    node_uuid: Option<String>,
}

/// Why a `RotateDek` is being proposed. Drives the recipient set:
/// `NodeRevoked` excludes the named node; everything else keeps every
/// active node.
#[derive(Debug, Clone)]
pub enum RotateReason {
    /// A node was revoked and the DEK must be rotated to exclude it.
    NodeRevoked {
        /// Cluster-wide UUID of the revoked node.
        node_id: String,
    },
    /// Periodic scheduled rotation (no nodes excluded).
    Scheduled,
    /// Operator-initiated rotation (no nodes excluded).
    Manual,
}

/// Post-apply node-effect dispatch for the state-machine apply wrapper.
///
/// Pure inspection of the `(Request, Response)` pair: fires the matching
/// notify on `effects` when an op that requires per-node side action
/// applies successfully. Exposed as a free function for unit testing —
/// the actual closure handed to the openraft state machine in
/// [`RaftCoordinator::with_auth_secrets_and_effects`] is a thin wrapper
/// around `state.apply(req)` + a call to this helper.
fn dispatch_node_effects(effects: Option<&NodeSideEffects>, req: &Request, resp: &Response) {
    if let (Some(effects), Request::Secrets(SecretsRaftOp::WipeJoinSecret)) = (effects, req) {
        if matches!(resp, Response::Success { .. }) {
            effects.fire_wipe_join_secret();
        }
    }
}

impl RaftCoordinator {
    /// Create a new Raft coordinator without auth.
    ///
    /// Discards [`BootstrapState`]. Use [`Self::with_auth_and_secrets`] directly
    /// when you need to know whether this is a fresh cluster or a resumption.
    ///
    /// # Errors
    ///
    /// Returns an error if the data directory cannot be created, or if the
    /// log store, state machine, or consensus node fails to initialize.
    pub async fn new(config: RaftConfig) -> Result<Self> {
        Ok(Self::with_auth_and_secrets(config, None, None, None)
            .await?
            .coordinator)
    }

    /// Create a new Raft coordinator with an optional bearer token for RPC auth.
    ///
    /// When `auth_token` is `Some`, the HTTP client will attach it to every
    /// outgoing Raft RPC as an `Authorization: Bearer <token>` header.
    ///
    /// This constructor leaves the secrets capability disabled. Use
    /// [`Self::with_auth_and_secrets`] when the daemon also needs to drive
    /// the cluster secrets state machine.
    ///
    /// Discards [`BootstrapState`]. Use [`Self::with_auth_and_secrets`] directly
    /// when you need to know whether this is a fresh cluster or a resumption.
    ///
    /// # Errors
    ///
    /// Returns an error if the data directory cannot be created, or if the
    /// log store, state machine, or consensus node fails to initialize.
    pub async fn with_auth(config: RaftConfig, auth_token: Option<String>) -> Result<Self> {
        Ok(Self::with_auth_and_secrets(config, auth_token, None, None)
            .await?
            .coordinator)
    }

    /// Create a new Raft coordinator with optional auth and optional secrets
    /// capability.
    ///
    /// `node_priv` and `node_uuid` enable the leader-side secrets
    /// orchestration helpers ([`Self::propose_register_node_and_rotate`],
    /// [`Self::propose_rotate_dek`]). They must either both be `Some` or
    /// both be `None`; if mismatched, the secrets helpers behave as if
    /// they were both `None` (returning a "no secrets capability" error
    /// from each helper).
    ///
    /// # Errors
    ///
    /// Returns an error if the data directory cannot be created, or if the
    /// log store, state machine, or consensus node fails to initialize.
    pub async fn with_auth_and_secrets(
        config: RaftConfig,
        auth_token: Option<String>,
        node_priv: Option<RecipientPrivateKey>,
        node_uuid: Option<String>,
    ) -> Result<CoordinatorInit> {
        Self::with_auth_secrets_and_effects(config, auth_token, node_priv, node_uuid, None).await
    }

    /// Like [`Self::with_auth_and_secrets`] but additionally accepts a
    /// [`NodeSideEffects`] handle. The apply wrapper consults this handle
    /// post-apply: when a `SecretsRaftOp::WipeJoinSecret` op applies
    /// successfully it calls [`NodeSideEffects::fire_wipe_join_secret`],
    /// waking the daemon's watcher to delete `{data_dir}/join_secret` on
    /// this node. `None` keeps the apply path effect-free (used by tests
    /// and the no-secrets-capability constructor shorthands).
    ///
    /// # Errors
    ///
    /// Same as [`Self::with_auth_and_secrets`].
    pub async fn with_auth_secrets_and_effects(
        config: RaftConfig,
        auth_token: Option<String>,
        node_priv: Option<RecipientPrivateKey>,
        node_uuid: Option<String>,
        node_effects: Option<Arc<NodeSideEffects>>,
    ) -> Result<CoordinatorInit> {
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

        // Detect whether raft state already exists on disk BEFORE opening
        // the redb files (since `redb::Database::create` will create the
        // files if absent, after which existence is no longer a signal).
        //
        // The redb crate writes the file header eagerly on `create`, so a
        // mere presence check is ambiguous; require non-zero length to
        // distinguish "openraft has actually persisted state" from "we
        // just opened an empty database in a prior aborted run".
        #[cfg(feature = "persistent")]
        let bootstrap_state = {
            let log_path = config.data_dir.join("raft-log");
            let sm_path = config.data_dir.join("raft-sm");
            let has_log = std::fs::metadata(&log_path)
                .map(|m| m.len() > 0)
                .unwrap_or(false);
            let has_sm = std::fs::metadata(&sm_path)
                .map(|m| m.len() > 0)
                .unwrap_or(false);
            if has_log || has_sm {
                BootstrapState::Resuming
            } else {
                BootstrapState::Fresh
            }
        };
        #[cfg(not(feature = "persistent"))]
        let bootstrap_state = BootstrapState::Fresh;

        // Apply wrapper: invokes the pure `ClusterState::apply` then dispatches
        // node-local side effects (e.g. filesystem delete of `join_secret` after
        // `WipeJoinSecret`). When `node_effects` is `None` the closure degenerates
        // to a thin wrapper over `state.apply`, matching the pre-effects behavior.
        let apply_fn = {
            let effects = node_effects.clone();
            move |state: &mut ClusterState, req: &Request| -> Response {
                let resp = state.apply(req);
                dispatch_node_effects(effects.as_deref(), req, &resp);
                resp
            }
        };

        // Create storage (persistent redb or in-memory depending on feature)
        #[cfg(feature = "persistent")]
        let (log_store, state_machine, sm_data) = {
            let log_path = config.data_dir.join("raft-log");
            let sm_path = config.data_dir.join("raft-sm");
            let log_store = RedbLogStore::<TypeConfig>::new(&log_path)
                .map_err(|e| SchedulerError::Raft(format!("Failed to open raft log store: {e}")))?;
            let state_machine = RedbStateMachine::<TypeConfig, ClusterState, _>::new(
                &sm_path, apply_fn,
            )
            .map_err(|e| SchedulerError::Raft(format!("Failed to open raft state machine: {e}")))?;
            let sm_data = state_machine.state();
            (log_store, state_machine, sm_data)
        };
        #[cfg(not(feature = "persistent"))]
        let (log_store, state_machine, sm_data) = {
            let log_store = MemLogStore::<TypeConfig>::default();
            let state_machine = MemStateMachine::<TypeConfig, ClusterState, _>::new(apply_fn);
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

        // Pair the secrets-capability inputs: only enable secrets helpers
        // when *both* the node private key and the node UUID are present.
        let (node_priv, node_uuid) = match (node_priv, node_uuid) {
            (Some(pk), Some(uuid)) => (Some(Arc::new(pk)), Some(uuid)),
            _ => (None, None),
        };

        Ok(CoordinatorInit {
            coordinator: Self {
                node,
                sm_data,
                config,
                node_priv,
                node_uuid,
            },
            bootstrap_state,
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
            os: params.os,
            arch: params.arch,
            slice_cidr: params.slice_cidr,
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

    // =========================================================================
    // Secrets state machine helpers
    //
    // These are leader-only orchestration entry points for the Raft-replicated
    // cluster secrets state machine. Followers should observe the secrets
    // state via [`Self::secrets_state`] and propose any writes through the
    // leader (typically by redirecting on the API layer using
    // [`Self::leader_id`] / [`Self::is_leader`]).
    //
    // Note on DEK rotation policy on `RegisterNode`: this helper *re-wraps the
    // existing DEK* for the new joiner rather than rotating to a fresh DEK on
    // every join. Re-wrapping is the cheap path (one sealed-box per node) and
    // does not require re-encrypting any existing `ReplicatedSecret` rows.
    // Genuine forward-secrecy concerns (e.g. node revocation) go through
    // [`Self::propose_rotate_dek`] which generates a fresh DEK and walks the
    // entire secrets table to re-encrypt every entry.
    // =========================================================================

    /// Read-only snapshot of the cluster secrets state.
    ///
    /// Returned by clone, which is fine because [`SecretsState`] is small —
    /// a few hash maps of identity / wrap / ciphertext bytes. Use this for
    /// follower-side reads in `RaftSecretsStore` (Task #12).
    pub async fn secrets_state(&self) -> SecretsState {
        let sm = self.sm_data.read().await;
        sm.state.secrets.clone()
    }

    /// Generic propose helper for an arbitrary [`SecretsRaftOp`].
    ///
    /// Used by the upcoming `RaftSecretsStore` (Task #12) for the simpler
    /// `PutSecret` / `DeleteSecret` paths that don't need DEK orchestration.
    ///
    /// # Errors
    /// - Returns [`SecretsError::Provider`] with a "not leader" message
    ///   when this node is not the current leader, so the API layer can
    ///   redirect to the leader's address surfaced via [`Self::leader_id`].
    /// - Returns [`SecretsError::Provider`] for any other Raft propose
    ///   failure (commit timeout, log append error, etc.).
    pub async fn propose_secrets_op(
        &self,
        op: SecretsRaftOp,
    ) -> std::result::Result<(), SecretsError> {
        if !self.node.is_leader() {
            return Err(SecretsError::Provider(format!(
                "not leader; redirect to: {}",
                self.node
                    .leader_id()
                    .map_or_else(|| "<unknown>".to_string(), |id| id.to_string(),),
            )));
        }
        match self.node.propose(Request::Secrets(op)).await {
            Ok(Response::Success { .. }) => Ok(()),
            Ok(Response::Error { message }) => Err(SecretsError::Provider(message)),
            Err(e) => Err(SecretsError::Provider(format!("Raft propose failed: {e}"))),
        }
    }

    /// Propose a [`SecretsRaftOp::PutSecret`].
    ///
    /// Leader-only. Followers receive a `not leader` error so the API
    /// layer can redirect.
    ///
    /// # Errors
    /// See [`Self::propose_secrets_op`].
    pub async fn propose_put_secret(
        &self,
        secret: ReplicatedSecret,
    ) -> std::result::Result<(), SecretsError> {
        self.propose_secrets_op(SecretsRaftOp::PutSecret { secret })
            .await
    }

    /// Propose a [`SecretsRaftOp::DeleteSecret`].
    ///
    /// Leader-only. Followers receive a `not leader` error so the API
    /// layer can redirect.
    ///
    /// # Errors
    /// See [`Self::propose_secrets_op`].
    pub async fn propose_delete_secret(
        &self,
        storage_key: &str,
    ) -> std::result::Result<(), SecretsError> {
        self.propose_secrets_op(SecretsRaftOp::DeleteSecret {
            storage_key: storage_key.to_string(),
        })
        .await
    }

    /// Propose a [`SecretsRaftOp::RegisterNode`] followed by a
    /// [`SecretsRaftOp::RotateDek`] re-wrap that includes the joining node.
    ///
    /// The re-wrap is performed against the *current* DEK when one already
    /// exists (no actual rotation), or a freshly generated DEK on the
    /// first registration. Returns the joiner's sealed-box wrap of the
    /// effective DEK plus the resulting `dek_generation`, which the
    /// cluster-join HTTP handler echoes back to the joining agent.
    ///
    /// # Errors
    /// - [`SecretsError::Provider`] when this node is not the leader, when
    ///   secrets capability was not enabled at construction (no
    ///   `node_priv` / `node_uuid`), when unwrapping the existing DEK
    ///   fails, or when any subsequent Raft propose fails.
    /// - [`SecretsError::Encryption`] when sealed-box wrapping fails for
    ///   any recipient.
    pub async fn propose_register_node_and_rotate(
        &self,
        identity: NodeIdentity,
    ) -> std::result::Result<(Vec<u8>, u64), SecretsError> {
        if !self.node.is_leader() {
            return Err(SecretsError::Provider(format!(
                "not leader; redirect to: {}",
                self.node
                    .leader_id()
                    .map_or_else(|| "<unknown>".to_string(), |id| id.to_string(),),
            )));
        }
        let leader_priv = self.node_priv.as_ref().ok_or_else(|| {
            SecretsError::Provider(
                "RaftCoordinator constructed without secrets capability \
                 (missing node_priv); cannot orchestrate RegisterNode + RotateDek"
                    .to_string(),
            )
        })?;
        let leader_uuid = self.node_uuid.as_ref().ok_or_else(|| {
            SecretsError::Provider(
                "RaftCoordinator constructed without secrets capability \
                 (missing node_uuid); cannot orchestrate RegisterNode + RotateDek"
                    .to_string(),
            )
        })?;

        // Snapshot current state. The lock is dropped before any await on
        // a Raft propose to avoid holding it across the wide network round
        // trip the apply will trigger.
        let current = self.secrets_state().await;

        // Determine the effective DEK: re-use the current one when one
        // already exists, else generate a fresh DEK. Re-wrap (no rotate)
        // keeps existing `ReplicatedSecret` rows valid under the same
        // generation.
        let (dek, base_generation) = match current.wrapped_dek.as_ref() {
            Some(envelope) => {
                let leader_wrap = envelope.wraps.get(leader_uuid).ok_or_else(|| {
                    SecretsError::Provider(format!(
                        "leader node_uuid {leader_uuid} has no wrap in current DEK \
                         (generation {}) — cluster cannot register a new node",
                        envelope.dek_generation
                    ))
                })?;
                let dek = ClusterDek::unwrap(leader_priv, leader_wrap)?;
                (dek, envelope.dek_generation)
            }
            None => (ClusterDek::generate(), 0),
        };

        // 1) RegisterNode — proposed first so the apply ordering matches
        //    the natural reading: the recipient set used by the subsequent
        //    `RotateDek` reflects the post-registration node table.
        self.propose_secrets_op(SecretsRaftOp::RegisterNode {
            identity: identity.clone(),
        })
        .await?;

        // 2) Build the recipient set (every active node + the joiner).
        //    The joiner is added explicitly because, depending on apply
        //    timing on the leader, the in-memory snapshot may still be
        //    pre-RegisterNode at this exact moment.
        let mut recipients: HashMap<String, RecipientPublicKey> = HashMap::new();
        for (node_id, node) in &current.nodes {
            if node.revoked_at.is_some() {
                continue;
            }
            recipients.insert(
                node_id.clone(),
                RecipientPublicKey::from_bytes(node.secrets_pubkey),
            );
        }
        recipients.insert(
            identity.node_id.clone(),
            RecipientPublicKey::from_bytes(identity.secrets_pubkey),
        );

        // 3) RotateDek with the same DEK material at generation
        //    `base_generation + 1`. Pure re-wrap — existing
        //    ReplicatedSecrets stay valid because the DEK bytes are
        //    unchanged (only the per-node sealed-box wraps differ).
        let new_generation = base_generation.saturating_add(1);
        let envelope = dek.rewrap_for_set(&recipients, new_generation)?;

        // Pull the joiner's wrap out before moving the envelope into
        // Raft. Cloned so the subsequent `Secrets(...)` propose can
        // serialize the full `WrappedDek` unchanged.
        let joiner_wrap = envelope
            .wraps
            .get(&identity.node_id)
            .cloned()
            .ok_or_else(|| {
                SecretsError::Provider(format!(
                    "internal: rewrap_for_set produced no wrap for joiner {}",
                    identity.node_id
                ))
            })?;

        self.propose_secrets_op(SecretsRaftOp::RotateDek {
            new_wraps: envelope,
        })
        .await?;

        Ok((joiner_wrap, new_generation))
    }

    /// Propose a [`SecretsRaftOp::RotateDek`] that generates a fresh DEK,
    /// re-wraps it for every active node (excluding any node named in
    /// [`RotateReason::NodeRevoked`]), and re-encrypts every existing
    /// [`ReplicatedSecret`] from the previous generation to the new one
    /// via a batch of [`SecretsRaftOp::PutSecret`] proposes.
    ///
    /// The previous DEK is unwrapped on the leader and held in a local
    /// variable for the duration of the re-encrypt batch, then dropped
    /// (zeroized) at the end of the call.
    ///
    /// Returns the new DEK generation.
    ///
    /// # Errors
    /// - [`SecretsError::Provider`] when this node is not the leader, when
    ///   secrets capability is disabled (no `node_priv` / `node_uuid`),
    ///   when there is no current DEK to rotate from, or when any Raft
    ///   propose fails.
    /// - [`SecretsError::Encryption`] / [`SecretsError::Decryption`] when
    ///   sealed-box (un)wrapping or AEAD (en|de)cryption fails.
    pub async fn propose_rotate_dek(
        &self,
        reason: RotateReason,
    ) -> std::result::Result<u64, SecretsError> {
        if !self.node.is_leader() {
            return Err(SecretsError::Provider(format!(
                "not leader; redirect to: {}",
                self.node
                    .leader_id()
                    .map_or_else(|| "<unknown>".to_string(), |id| id.to_string(),),
            )));
        }
        let leader_priv = self.node_priv.as_ref().ok_or_else(|| {
            SecretsError::Provider(
                "RaftCoordinator constructed without secrets capability \
                 (missing node_priv); cannot orchestrate RotateDek"
                    .to_string(),
            )
        })?;
        let leader_uuid = self.node_uuid.as_ref().ok_or_else(|| {
            SecretsError::Provider(
                "RaftCoordinator constructed without secrets capability \
                 (missing node_uuid); cannot orchestrate RotateDek"
                    .to_string(),
            )
        })?;

        // Snapshot the pre-rotation state — both the DEK envelope (for
        // unwrapping) and the secrets table (for re-encrypt).
        let current = self.secrets_state().await;
        let previous_envelope = current.wrapped_dek.as_ref().ok_or_else(|| {
            SecretsError::Provider(
                "no current DEK to rotate from; call \
                 propose_register_node_and_rotate first"
                    .to_string(),
            )
        })?;
        let previous_dek = {
            let leader_wrap = previous_envelope.wraps.get(leader_uuid).ok_or_else(|| {
                SecretsError::Provider(format!(
                    "leader node_uuid {leader_uuid} has no wrap in current DEK \
                     (generation {}) — cannot rotate",
                    previous_envelope.dek_generation
                ))
            })?;
            ClusterDek::unwrap(leader_priv, leader_wrap)?
        };
        let previous_generation = previous_envelope.dek_generation;

        // Build the post-rotation recipient set. Any node whose
        // `revoked_at` is set or that matches `RotateReason::NodeRevoked`
        // is excluded from the new wraps and from any future re-encrypts.
        let revoked_id = match &reason {
            RotateReason::NodeRevoked { node_id } => Some(node_id.as_str()),
            _ => None,
        };
        let mut recipients: HashMap<String, RecipientPublicKey> = HashMap::new();
        for (node_id, node) in &current.nodes {
            if node.revoked_at.is_some() {
                continue;
            }
            if Some(node_id.as_str()) == revoked_id {
                continue;
            }
            recipients.insert(
                node_id.clone(),
                RecipientPublicKey::from_bytes(node.secrets_pubkey),
            );
        }

        // Generate the new DEK and propose the rotation.
        let new_dek = ClusterDek::generate();
        let new_generation = previous_generation.saturating_add(1);
        let new_envelope = new_dek.rewrap_for_set(&recipients, new_generation)?;
        self.propose_secrets_op(SecretsRaftOp::RotateDek {
            new_wraps: new_envelope,
        })
        .await?;

        // Re-encrypt every secret on the old generation and propose
        // PutSecret updates one at a time. Followers apply each update
        // atomically; partial-progress on leader failure is recoverable
        // because the rows already on the new generation stay valid and
        // the leftover rows can be picked up by a subsequent rotation.
        for secret in current.secrets.values() {
            if secret.dek_generation == new_generation {
                continue;
            }
            // Decrypt under the old DEK, re-encrypt under the new one.
            let plaintext = previous_dek.decrypt(&secret.ciphertext)?;
            let new_ciphertext = new_dek.encrypt(plaintext.as_slice())?;
            let mut updated = secret.clone();
            updated.ciphertext = new_ciphertext;
            updated.dek_generation = new_generation;
            self.propose_secrets_op(SecretsRaftOp::PutSecret { secret: updated })
                .await?;
        }

        // `previous_dek` and `new_dek` go out of scope here and are
        // zeroized by `Drop`.
        info!(
            previous_generation,
            new_generation,
            reason = ?reason,
            "Rotated cluster DEK and re-encrypted secrets",
        );
        Ok(new_generation)
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

    /// Trigger an immediate election on this node.
    ///
    /// Used by the leader's pre-self-upgrade orchestration: the leader
    /// picks a healthy follower and asks it to campaign right away,
    /// instead of waiting for heartbeat-loss to expire after the leader
    /// drops. Raft safety guarantees that only an up-to-date candidate
    /// can win, so the worst case is a single failed election round.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `Raft::trigger().elect()` call
    /// reports a fatal coordinator error (shutdown, storage failure).
    pub async fn trigger_elect(&self) -> Result<()> {
        self.node
            .raft()
            .trigger()
            .elect()
            .await
            .map_err(|e| SchedulerError::Raft(format!("trigger_elect failed: {e}")))?;
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

// `RaftSecretsHandle` impl: lets `zlayer_secrets::RaftSecretsStore`
// drive cluster-mode secrets reads/writes through us without
// `zlayer-secrets` needing to depend on `zlayer-scheduler` (that edge
// would close a cycle — `zlayer-scheduler -> zlayer-secrets` already
// exists for crypto + SM types). The trait body is just thin
// adapters over the existing `secrets_state` / `propose_*` methods.
#[async_trait::async_trait]
impl zlayer_secrets::RaftSecretsHandle for RaftCoordinator {
    async fn secrets_state(&self) -> SecretsState {
        // Delegate to the inherent method by full path so the trait
        // method and the inherent method don't shadow each other.
        Self::secrets_state(self).await
    }

    async fn propose_put_secret(
        &self,
        secret: ReplicatedSecret,
    ) -> std::result::Result<(), SecretsError> {
        Self::propose_put_secret(self, secret).await
    }

    async fn propose_delete_secret(
        &self,
        storage_key: &str,
    ) -> std::result::Result<(), SecretsError> {
        Self::propose_delete_secret(self, storage_key).await
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
    fn secrets_raft_op_roundtrips_through_postcard() {
        use zlayer_types::api::internal::SecretsRaftOp;

        let cases = [
            SecretsRaftOp::RevokeNode {
                node_id: "n1".into(),
            },
            SecretsRaftOp::WipeJoinSecret,
            SecretsRaftOp::DeleteSecret {
                storage_key: "scope:key".into(),
            },
        ];
        for op in &cases {
            let bytes = postcard2::to_vec(op).expect("encode");
            let back: SecretsRaftOp = postcard2::from_bytes(&bytes).expect("decode");
            let _ = format!("{back:?}");
        }
    }

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
            os: None,
            arch: None,
            slice_cidr: String::new(),
        });

        let node = state.get_node(1).unwrap();
        assert_eq!(node.address, "192.168.1.1:8000");
        assert_eq!(node.cpu_total, 8.0);
        assert_eq!(node.memory_total, 16_000_000_000);
        assert_eq!(node.status, "ready");
    }

    #[test]
    fn update_node_mode_changes_node_info() {
        let mut state = ClusterState::new();

        state.apply(&Request::RegisterNode {
            node_id: 7,
            address: "10.0.0.7:9000".to_string(),
            wg_public_key: String::new(),
            overlay_ip: String::new(),
            overlay_port: 0,
            advertise_addr: String::new(),
            api_port: 0,
            cpu_total: 4.0,
            memory_total: 8_000_000_000,
            disk_total: 100_000_000_000,
            gpus: vec![],
            mode: "full".to_string(),
            os: None,
            arch: None,
            slice_cidr: String::new(),
        });

        let resp = state.apply(&Request::UpdateNodeMode {
            node_id: 7,
            mode: "replicate".to_string(),
        });
        assert!(matches!(resp, Response::Success { .. }));
        assert_eq!(state.get_node(7).unwrap().mode, "replicate");

        // Invalid mode rejected, NodeInfo unchanged.
        let bad = state.apply(&Request::UpdateNodeMode {
            node_id: 7,
            mode: "bogus".to_string(),
        });
        assert!(matches!(bad, Response::Error { .. }));
        assert_eq!(state.get_node(7).unwrap().mode, "replicate");

        // Unknown node id surfaces as an error.
        let missing = state.apply(&Request::UpdateNodeMode {
            node_id: 999,
            mode: "full".to_string(),
        });
        assert!(matches!(missing, Response::Error { .. }));
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
        let dir = ZLayerDirs::system_default()
            .scratch_dir("test-force-leader-state-save-load-")
            .unwrap();
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
            os: None,
            arch: None,
            slice_cidr: String::new(),
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
        let dir = ZLayerDirs::system_default()
            .scratch_dir("test-force-leader-state-save-load-ipv6-overlay-")
            .unwrap();
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
            os: None,
            arch: None,
            slice_cidr: String::new(),
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
            os: None,
            arch: None,
            slice_cidr: String::new(),
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
        let dir = ZLayerDirs::system_default()
            .scratch_dir("test-force-leader-no-marker-")
            .unwrap();
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

    // -- Secrets dispatch --------------------------------------------------

    #[test]
    fn cluster_state_secrets_dispatch_register_node() {
        // `Request::Secrets(...)` should be applied by the existing
        // `ClusterState::apply` to the inner `SecretsState`. Verify the
        // RegisterNode op lands in `state.secrets.nodes` without disturbing
        // any of the pre-existing scheduler tables.
        let mut state = ClusterState::new();

        let identity = NodeIdentity {
            node_id: "node-uuid-a".to_string(),
            secrets_pubkey: [1u8; 32],
            wg_pubkey: "wg-pub-a".to_string(),
            joined_at: chrono::Utc::now(),
            revoked_at: None,
        };

        let resp = state.apply(&Request::Secrets(SecretsRaftOp::RegisterNode {
            identity: identity.clone(),
        }));
        assert!(matches!(resp, Response::Success { .. }));
        assert!(state.secrets.nodes.contains_key("node-uuid-a"));
        assert!(state.services.is_empty());
        assert!(state.nodes.is_empty());
    }

    #[test]
    fn cluster_state_secrets_dispatch_propagates_inner_error() {
        // `SecretsState::apply` returns an error for unknown deletes;
        // the dispatcher should surface that as `Response::Error` so the
        // proposer can see what happened, while the SM stays converged
        // (no panics, deterministic outcome on every replica).
        let mut state = ClusterState::new();
        let resp = state.apply(&Request::Secrets(SecretsRaftOp::DeleteSecret {
            storage_key: "dep:nope".to_string(),
        }));
        match resp {
            Response::Error { message } => {
                assert!(message.contains("nope"), "unexpected message: {message}");
            }
            other @ Response::Success { .. } => {
                panic!("expected Response::Error, got {other:?}")
            }
        }
    }

    #[test]
    fn cluster_state_snapshot_backcompat_missing_secrets_field() {
        // Pre-secrets snapshots have no `secrets` field. The new field
        // is `#[serde(default)]` so they must deserialize cleanly with
        // an empty SecretsState.
        let legacy_json = r#"{
            "services": {},
            "nodes": {},
            "scale_events": []
        }"#;
        let restored: ClusterState =
            serde_json::from_str(legacy_json).expect("legacy snapshot deserializes");
        assert!(restored.services.is_empty());
        assert!(restored.nodes.is_empty());
        assert!(restored.secrets.nodes.is_empty());
        assert!(restored.secrets.wrapped_dek.is_none());
        assert!(restored.secrets.secrets.is_empty());
    }

    #[test]
    fn rotate_reason_node_revoked_carries_id() {
        // Sanity check on the `RotateReason::NodeRevoked` payload shape
        // so future changes don't accidentally drop the `node_id` field
        // (the leader-side rotation orchestration relies on it to filter
        // the recipient set).
        let r = RotateReason::NodeRevoked {
            node_id: "revoked-uuid".to_string(),
        };
        match r {
            RotateReason::NodeRevoked { node_id } => assert_eq!(node_id, "revoked-uuid"),
            RotateReason::Scheduled | RotateReason::Manual => panic!("wrong variant"),
        }
    }

    /// Constructing a coordinator twice in the same data directory must
    /// report `Fresh` on the first open (no files on disk) and `Resuming`
    /// on the second open (after `bootstrap()` has persisted state).
    ///
    /// This is the signal `daemon.rs` uses to decide whether to call
    /// `bootstrap()` — the previous `metrics.current_term > 0` probe was
    /// racy because the freshly constructed `RaftCore` hadn't finished its
    /// async state-load when the daemon probed it, so we'd incorrectly
    /// re-bootstrap and trigger openraft's internal
    /// `error!("Can not initialize ...")` line.
    #[cfg(feature = "persistent")]
    #[tokio::test]
    async fn coordinator_reports_fresh_then_resuming() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = RaftConfig {
            node_id: 1,
            address: "127.0.0.1:0".to_string(),
            data_dir: tmp.path().to_path_buf(),
            ..RaftConfig::default()
        };

        // First open: no files on disk → Fresh.
        let init1 = RaftCoordinator::with_auth_and_secrets(config.clone(), None, None, None)
            .await
            .expect("first coordinator construction");
        assert_eq!(init1.bootstrap_state, BootstrapState::Fresh);

        // Trigger a disk write that populates raft-log/raft-sm. Calling
        // `bootstrap()` writes the initial membership entry.
        init1
            .coordinator
            .bootstrap()
            .await
            .expect("bootstrap on fresh coordinator");
        init1
            .coordinator
            .shutdown()
            .await
            .expect("shutdown first coordinator");
        drop(init1);
        // Give background tasks (raft tick loop, state-machine worker) a beat
        // to release the redb file lock. Redb releases the OS-level lock when
        // its last in-process handle drops; the consensus node holds those
        // handles inside spawned tasks that finish a tick or two after
        // `shutdown()` returns.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Second open: files exist and are non-empty → Resuming.
        let init2 = RaftCoordinator::with_auth_and_secrets(config, None, None, None)
            .await
            .expect("second coordinator construction");
        assert_eq!(init2.bootstrap_state, BootstrapState::Resuming);
        init2.coordinator.shutdown().await.ok();
    }

    #[tokio::test]
    async fn dispatch_node_effects_fires_on_successful_wipe_join_secret() {
        use std::sync::Arc;
        use zlayer_secrets::NodeSideEffects;
        use zlayer_types::api::internal::SecretsRaftOp;

        let effects = NodeSideEffects::new();
        let cloned = Arc::clone(&effects);
        let waiter = tokio::spawn(async move {
            cloned.wait_wipe_join_secret().await;
        });
        tokio::task::yield_now().await;

        super::dispatch_node_effects(
            Some(effects.as_ref()),
            &Request::Secrets(SecretsRaftOp::WipeJoinSecret),
            &Response::Success { data: None },
        );

        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("waiter must wake within 1s of fire")
            .expect("waiter task did not panic");
    }

    #[tokio::test]
    async fn dispatch_node_effects_does_not_fire_on_error_response() {
        use std::sync::Arc;
        use zlayer_secrets::NodeSideEffects;
        use zlayer_types::api::internal::SecretsRaftOp;

        let effects = NodeSideEffects::new();
        let cloned = Arc::clone(&effects);
        let waiter = tokio::spawn(async move {
            cloned.wait_wipe_join_secret().await;
        });
        tokio::task::yield_now().await;

        super::dispatch_node_effects(
            Some(effects.as_ref()),
            &Request::Secrets(SecretsRaftOp::WipeJoinSecret),
            &Response::Error {
                message: "simulated".into(),
            },
        );

        // Notify must NOT have fired — waiter should still be parked.
        let result = tokio::time::timeout(std::time::Duration::from_millis(100), waiter).await;
        assert!(
            result.is_err(),
            "dispatch_node_effects must not fire on Response::Error",
        );
    }

    #[tokio::test]
    async fn dispatch_node_effects_does_not_fire_for_other_ops() {
        use std::sync::Arc;
        use zlayer_secrets::NodeSideEffects;
        use zlayer_types::api::internal::SecretsRaftOp;

        let effects = NodeSideEffects::new();
        let cloned = Arc::clone(&effects);
        let waiter = tokio::spawn(async move {
            cloned.wait_wipe_join_secret().await;
        });
        tokio::task::yield_now().await;

        // A different SecretsRaftOp — should not fire.
        super::dispatch_node_effects(
            Some(effects.as_ref()),
            &Request::Secrets(SecretsRaftOp::DeleteSecret {
                storage_key: "dep:nope".into(),
            }),
            &Response::Success { data: None },
        );

        let result = tokio::time::timeout(std::time::Duration::from_millis(100), waiter).await;
        assert!(
            result.is_err(),
            "dispatch_node_effects must only fire for WipeJoinSecret",
        );
    }

    #[test]
    fn grant_renew_expire_lease_round_trip() {
        let mut state = ClusterState::new();
        let now_ns: u64 = 1_700_000_000_000_000_000;

        let resp = state.apply(&Request::GrantWorkerLease {
            node_id: 42,
            holder: "worker-cn-1".into(),
            acquired_ns: now_ns,
            renewed_ns: now_ns,
            ttl_secs: 60,
        });
        assert!(matches!(resp, Response::Success { .. }));
        assert!(state.worker_leases.contains_key(&42));

        // Renew advances renewed_ns and increments revision.
        let later = now_ns + 30_000_000_000;
        state.apply(&Request::RenewWorkerLease {
            node_id: 42,
            renewed_ns: later,
            ttl_secs: 90,
        });
        let l = state.worker_leases.get(&42).expect("present");
        assert_eq!(l.renewed_ns, later);
        assert_eq!(l.ttl_secs, 90);
        assert_eq!(l.revision, 2);

        // Stale renew (renewed_ns < current) is ignored.
        let stale_renewed_ns = now_ns - 1_000_000_000;
        state.apply(&Request::RenewWorkerLease {
            node_id: 42,
            renewed_ns: stale_renewed_ns,
            ttl_secs: 10,
        });
        let l = state.worker_leases.get(&42).expect("still present");
        assert_eq!(l.renewed_ns, later); // unchanged
        assert_eq!(l.ttl_secs, 90); // unchanged

        // Expire removes.
        state.apply(&Request::ExpireWorkerLease { node_id: 42 });
        assert!(!state.worker_leases.contains_key(&42));

        // Idempotent expire = no-op.
        let resp2 = state.apply(&Request::ExpireWorkerLease { node_id: 42 });
        assert!(matches!(resp2, Response::Success { .. }));
    }

    #[test]
    fn worker_lease_is_expired_math() {
        let rec = WorkerLeaseRecord {
            node_id: 1,
            holder: "x".into(),
            acquired_ns: 1_000_000_000,
            renewed_ns: 1_000_000_000,
            ttl_secs: 10,
            revision: 1,
        };
        // 10s TTL + 0 grace; not expired at +5s, expired at +10s.
        assert!(!rec.is_expired(1_000_000_000 + 5_000_000_000, 0));
        assert!(rec.is_expired(1_000_000_000 + 10_000_000_000, 0));
        // With 5s grace: not expired at +10s, expired at +15s.
        assert!(!rec.is_expired(1_000_000_000 + 10_000_000_000, 5));
        assert!(rec.is_expired(1_000_000_000 + 15_000_000_000, 5));
    }

    #[test]
    fn old_snapshot_without_worker_leases_field_deserializes() {
        // Simulates a snapshot serialized before worker_leases existed.
        // serde_json equivalent of legacy ClusterState (no worker_leases key).
        let legacy = serde_json::json!({
            "services": {},
            "nodes": {},
            "scale_events": []
            // worker_leases intentionally absent
            // secrets intentionally absent (has #[serde(default)] already)
        });
        let s: ClusterState = serde_json::from_value(legacy).expect("legacy deserialize");
        assert!(s.worker_leases.is_empty());
    }
}
