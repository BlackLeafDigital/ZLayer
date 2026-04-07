//! Node placement logic for `ZLayer` scheduler
//!
//! This module provides placement algorithms for distributing service replicas
//! across nodes based on different node allocation modes:
//!
//! - **Shared**: Containers bin-packed onto nodes with available capacity
//! - **Dedicated**: Each replica gets its own node (1:1 mapping)
//! - **Exclusive**: Service has nodes exclusively to itself (no other services)

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tracing::debug;
use zlayer_spec::{GpuSharingMode, NodeMode, NodeSelector, ServiceSpec};

use crate::error::{Result, SchedulerError};
use crate::raft::NodeId;

/// Unique identifier for a container instance
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContainerId {
    /// Service name this container belongs to
    pub service: String,
    /// Replica index (0-based)
    pub replica: u32,
}

impl ContainerId {
    /// Create a new container ID
    pub fn new(service: impl Into<String>, replica: u32) -> Self {
        Self {
            service: service.into(),
            replica,
        }
    }
}

impl std::fmt::Display for ContainerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-{}", self.service, self.replica)
    }
}

/// Allocation state of a single GPU
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum GpuAllocation {
    /// GPU is free
    #[default]
    Free,
    /// GPU is exclusively allocated to one container
    Exclusive,
    /// GPU is shared via MPS or time-slicing; tracks current share count and max
    Shared {
        /// Number of containers currently sharing this GPU
        current: u32,
        /// Maximum number of containers allowed to share
        max: u32,
    },
}

impl GpuAllocation {
    /// Returns true if this GPU can accept another container
    #[must_use]
    pub fn is_available(&self) -> bool {
        match self {
            Self::Free => true,
            Self::Exclusive => false,
            Self::Shared { current, max } => current < max,
        }
    }

    /// Returns true if this GPU is completely free
    #[must_use]
    pub fn is_free(&self) -> bool {
        matches!(self, Self::Free)
    }
}

/// Resource availability on a node
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NodeResources {
    /// Total CPU cores available
    pub cpu_total: f64,
    /// Used CPU cores
    pub cpu_used: f64,
    /// Total memory in bytes
    pub memory_total: u64,
    /// Used memory in bytes
    pub memory_used: u64,
    /// Total GPUs on this node
    pub gpu_total: u32,
    /// GPUs currently allocated to containers
    pub gpu_used: u32,
    /// GPU model names (e.g., `["NVIDIA A100-SXM4-80GB"]`)
    pub gpu_models: Vec<String>,
    /// Total GPU VRAM in MB across all GPUs.
    ///
    /// **Apple Silicon unified memory note**: On Apple Silicon (`gpu_vendor == "apple"`),
    /// GPU memory is physically the same as system RAM (unified architecture). This field
    /// will mirror a portion of `memory_total` rather than representing additional memory.
    /// Use [`NodeResources::is_unified_memory`] to detect this case and avoid double-counting
    /// when summing CPU + GPU memory budgets.
    pub gpu_memory_mb: u64,
    /// GPU vendor (e.g., "nvidia", "amd", "intel", "apple"), empty if no GPUs
    pub gpu_vendor: String,
    /// Per-GPU allocation state. Index corresponds to GPU detection order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gpu_allocated: Vec<GpuAllocation>,
}

impl NodeResources {
    /// Create new node resources
    #[must_use]
    pub fn new(cpu_total: f64, memory_total: u64) -> Self {
        Self {
            cpu_total,
            cpu_used: 0.0,
            memory_total,
            memory_used: 0,
            gpu_total: 0,
            gpu_used: 0,
            gpu_models: Vec::new(),
            gpu_memory_mb: 0,
            gpu_vendor: String::new(),
            gpu_allocated: Vec::new(),
        }
    }

    /// Get available CPU cores
    #[must_use]
    pub fn cpu_available(&self) -> f64 {
        (self.cpu_total - self.cpu_used).max(0.0)
    }

    /// Get available memory in bytes
    #[must_use]
    pub fn memory_available(&self) -> u64 {
        self.memory_total.saturating_sub(self.memory_used)
    }

    /// Calculate CPU utilization as a percentage
    #[must_use]
    pub fn cpu_utilization(&self) -> f64 {
        if self.cpu_total > 0.0 {
            (self.cpu_used / self.cpu_total) * 100.0
        } else {
            0.0
        }
    }

    /// Calculate memory utilization as a percentage
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn memory_utilization(&self) -> f64 {
        if self.memory_total > 0 {
            (self.memory_used as f64 / self.memory_total as f64) * 100.0
        } else {
            0.0
        }
    }

    /// Calculate overall utilization (average of CPU and memory)
    #[must_use]
    pub fn utilization(&self) -> f64 {
        f64::midpoint(self.cpu_utilization(), self.memory_utilization())
    }

    /// Get number of GPUs available for allocation
    ///
    /// A GPU is considered available if it is [`GpuAllocation::Free`] or if it
    /// is [`GpuAllocation::Shared`] with room for another container.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)] // GPU count fits in u32
    pub fn gpu_available(&self) -> u32 {
        if self.gpu_allocated.is_empty() {
            // Fallback for nodes that haven't reported per-GPU state yet
            self.gpu_total.saturating_sub(self.gpu_used)
        } else {
            self.gpu_allocated
                .iter()
                .filter(|a| a.is_available())
                .count() as u32
        }
    }

    /// Allocate `count` GPU indices, marking them according to the sharing mode.
    ///
    /// - `None` or `Some(Exclusive)`: sets [`GpuAllocation::Exclusive`] (only free GPUs)
    /// - `Some(Mps)`: sets [`GpuAllocation::Shared`] with `max = 8`, or increments an
    ///   existing shared GPU's `current` count
    /// - `Some(TimeSlice)`: sets [`GpuAllocation::Shared`] with `max = 4`, or increments
    ///   an existing shared GPU's `current` count
    ///
    /// Returns the allocated indices, or `None` if insufficient GPUs are available.
    #[allow(clippy::cast_possible_truncation)] // GPU index fits in u32
    pub fn allocate_gpus(
        &mut self,
        count: u32,
        sharing: Option<GpuSharingMode>,
    ) -> Option<Vec<u32>> {
        let mut indices = Vec::with_capacity(count as usize);
        for (i, alloc) in self.gpu_allocated.iter().enumerate() {
            if alloc.is_available() {
                indices.push(i as u32);
                if indices.len() == count as usize {
                    break;
                }
            }
        }
        if indices.len() == count as usize {
            for &idx in &indices {
                let slot = &mut self.gpu_allocated[idx as usize];
                match sharing {
                    None | Some(GpuSharingMode::Exclusive) => {
                        *slot = GpuAllocation::Exclusive;
                    }
                    Some(GpuSharingMode::Mps) => match slot {
                        GpuAllocation::Free => {
                            *slot = GpuAllocation::Shared { current: 1, max: 8 };
                        }
                        GpuAllocation::Shared { current, .. } => {
                            *current += 1;
                        }
                        GpuAllocation::Exclusive => {
                            // Should not happen since is_available() returned true,
                            // but be defensive
                        }
                    },
                    Some(GpuSharingMode::TimeSlice) => match slot {
                        GpuAllocation::Free => {
                            *slot = GpuAllocation::Shared { current: 1, max: 4 };
                        }
                        GpuAllocation::Shared { current, .. } => {
                            *current += 1;
                        }
                        GpuAllocation::Exclusive => {}
                    },
                }
            }
            // Keep gpu_used in sync for backward compat: count GPUs that are not free
            self.gpu_used = self.gpu_allocated.iter().filter(|a| !a.is_free()).count() as u32;
            Some(indices)
        } else {
            None
        }
    }

    /// Returns `true` if this node uses unified memory (Apple Silicon).
    ///
    /// On Apple Silicon, GPU VRAM and system RAM are the same physical memory pool.
    /// This means `gpu_memory_mb` is NOT additive with `memory_total` -- they overlap.
    /// Callers must not double-count memory when both CPU and GPU resources are requested
    /// on a unified-memory node.
    #[must_use]
    pub fn is_unified_memory(&self) -> bool {
        self.gpu_vendor.eq_ignore_ascii_case("apple")
    }

    /// Returns the total effective memory in MB, correctly handling unified memory.
    ///
    /// - **Discrete GPU nodes** (NVIDIA, AMD, Intel): system RAM + GPU VRAM
    /// - **Unified memory nodes** (Apple Silicon): system RAM only (GPU VRAM is a subset)
    #[must_use]
    pub fn total_effective_memory_mb(&self) -> u64 {
        let system_mb = self.memory_total / (1024 * 1024);
        if self.is_unified_memory() {
            // Apple Silicon: GPU VRAM is carved from system RAM, don't add it
            system_mb
        } else {
            // Discrete GPUs: VRAM is separate from system RAM
            system_mb + self.gpu_memory_mb
        }
    }
}

/// State of a node in the cluster
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeState {
    /// Node identifier
    pub id: NodeId,
    /// Node address
    pub address: String,
    /// Labels assigned to this node
    pub labels: HashMap<String, String>,
    /// Resource availability
    pub resources: NodeResources,
    /// Whether the node is healthy and available for placement
    pub healthy: bool,
}

impl NodeState {
    /// Create a new node state
    pub fn new(id: NodeId, address: impl Into<String>) -> Self {
        Self {
            id,
            address: address.into(),
            labels: HashMap::new(),
            resources: NodeResources::default(),
            healthy: true,
        }
    }

    /// Add a label to the node
    #[must_use]
    pub fn with_label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.labels.insert(key.into(), value.into());
        self
    }

    /// Set resources for the node
    #[must_use]
    pub fn with_resources(mut self, resources: NodeResources) -> Self {
        self.resources = resources;
        self
    }

    /// Get overall utilization percentage
    #[must_use]
    pub fn utilization(&self) -> f64 {
        self.resources.utilization()
    }

    /// Check if this node matches the required labels in a `NodeSelector`
    #[must_use]
    pub fn matches_required_labels(&self, selector: &NodeSelector) -> bool {
        selector
            .labels
            .iter()
            .all(|(key, value)| self.labels.get(key) == Some(value))
    }

    /// Count how many preferred labels this node matches
    #[must_use]
    pub fn preferred_label_score(&self, selector: &NodeSelector) -> usize {
        selector
            .prefer_labels
            .iter()
            .filter(|(key, value)| self.labels.get(*key) == Some(*value))
            .count()
    }
}

/// Placement decision for a container
#[derive(Debug, Clone)]
pub struct PlacementDecision {
    /// Container being placed
    pub container_id: ContainerId,
    /// Node to place the container on (None if no suitable node found)
    pub node_id: Option<NodeId>,
    /// Reason for the placement decision
    pub reason: PlacementReason,
    /// GPU indices allocated on the target node (empty if no GPUs requested)
    pub gpu_indices: Vec<u32>,
}

impl PlacementDecision {
    /// Check if placement was successful
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.node_id.is_some()
    }
}

/// Reason for a placement decision
#[derive(Debug, Clone)]
pub enum PlacementReason {
    /// Container was bin-packed onto a node
    BinPacked {
        /// Current utilization of the selected node
        node_utilization: f64,
    },
    /// Container placed on a dedicated node
    DedicatedNode,
    /// Container placed on an exclusive node
    ExclusiveNode,
    /// Node was selected based on label matching
    LabelMatch {
        /// Labels that matched
        matched_labels: Vec<String>,
    },
    /// No suitable node could be found
    NoSuitableNode {
        /// Explanation of why no node was suitable
        reason: String,
    },
}

/// Track current container placements on nodes
#[derive(Debug, Clone, Default)]
pub struct PlacementState {
    /// Mapping from node ID to containers placed on that node
    pub node_containers: HashMap<NodeId, Vec<ContainerId>>,
    /// Mapping from container ID to its assigned node
    pub container_nodes: HashMap<ContainerId, NodeId>,
}

impl PlacementState {
    /// Create a new empty placement state
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a container placement on a node
    pub fn place(&mut self, container_id: ContainerId, node_id: NodeId) {
        self.node_containers
            .entry(node_id)
            .or_default()
            .push(container_id.clone());
        self.container_nodes.insert(container_id, node_id);
    }

    /// Get containers on a specific node
    #[must_use]
    pub fn containers_on_node(&self, node_id: NodeId) -> &[ContainerId] {
        self.node_containers
            .get(&node_id)
            .map_or(&[], std::vec::Vec::as_slice)
    }

    /// Get the node for a specific container
    #[must_use]
    pub fn node_for_container(&self, container_id: &ContainerId) -> Option<NodeId> {
        self.container_nodes.get(container_id).copied()
    }

    /// Check if a node has any containers from a specific service
    #[must_use]
    pub fn has_service_on_node(&self, node_id: NodeId, service_name: &str) -> bool {
        self.containers_on_node(node_id)
            .iter()
            .any(|c| c.service == service_name)
    }

    /// Check if a node has any containers at all
    #[must_use]
    pub fn has_any_containers(&self, node_id: NodeId) -> bool {
        !self.containers_on_node(node_id).is_empty()
    }

    /// Get the count of containers on a node
    #[must_use]
    pub fn container_count(&self, node_id: NodeId) -> usize {
        self.containers_on_node(node_id).len()
    }

    /// Remove all container placements from a specific node.
    ///
    /// Returns the list of containers that were removed. This is used during
    /// node death handling to clear stale placements before rescheduling.
    pub fn remove_node(&mut self, node_id: NodeId) -> Vec<ContainerId> {
        let removed = self.node_containers.remove(&node_id).unwrap_or_default();
        for container in &removed {
            self.container_nodes.remove(container);
        }
        removed
    }

    /// Remove all containers for a specific service from a specific node.
    ///
    /// Returns the containers that were removed.
    pub fn remove_service_from_node(
        &mut self,
        node_id: NodeId,
        service_name: &str,
    ) -> Vec<ContainerId> {
        let mut removed = Vec::new();

        if let Some(containers) = self.node_containers.get_mut(&node_id) {
            let mut i = 0;
            while i < containers.len() {
                if containers[i].service == service_name {
                    let c = containers.swap_remove(i);
                    self.container_nodes.remove(&c);
                    removed.push(c);
                } else {
                    i += 1;
                }
            }
            // Clean up the node entry if no containers remain
            if containers.is_empty() {
                self.node_containers.remove(&node_id);
            }
        }

        removed
    }
}

/// Check if a node can accept a service based on `node_mode`, constraints, and resource availability
///
/// # Arguments
/// * `node` - The node to check
/// * `service_name` - Name of the service being placed
/// * `node_mode` - The node allocation mode for the service
/// * `node_selector` - Optional node selection constraints
/// * `placements` - Current placement state
/// * `service_spec` - Optional service spec for resource-aware checks (GPU requirements)
///
/// # Returns
/// `true` if the node can accept a replica of the service
pub fn can_place_on_node(
    node: &NodeState,
    service_name: &str,
    node_mode: NodeMode,
    node_selector: Option<&NodeSelector>,
    placements: &PlacementState,
    service_spec: Option<&ServiceSpec>,
) -> bool {
    // Node must be healthy
    if !node.healthy {
        return false;
    }

    // Check node selector labels if provided
    if let Some(selector) = node_selector {
        if !node.matches_required_labels(selector) {
            return false;
        }
    }

    // Check GPU requirements if the service requests GPUs
    if let Some(spec) = service_spec {
        if let Some(ref gpu) = spec.resources.gpu {
            let available = node.resources.gpu_available();
            if available < gpu.count {
                debug!(
                    node = %node.id,
                    gpu_requested = gpu.count,
                    gpu_available = available,
                    "Node rejected: insufficient GPUs"
                );
                return false;
            }
            // If vendor is specified and node has GPUs, check vendor matches
            if !gpu.vendor.is_empty()
                && !node.resources.gpu_vendor.is_empty()
                && node.resources.gpu_vendor != gpu.vendor
            {
                debug!(
                    node = %node.id,
                    requested_vendor = %gpu.vendor,
                    node_vendor = %node.resources.gpu_vendor,
                    "Node rejected: GPU vendor mismatch"
                );
                return false;
            }

            // Check GPU model affinity if requested
            if let Some(ref requested_model) = gpu.model {
                let has_matching_model = node
                    .resources
                    .gpu_models
                    .iter()
                    .any(|m| m.to_lowercase().contains(&requested_model.to_lowercase()));
                if !has_matching_model {
                    debug!(
                        node = %node.id,
                        requested_model = %requested_model,
                        available_models = ?node.resources.gpu_models,
                        "Node rejected: no GPU matching requested model"
                    );
                    return false;
                }
            }

            // Apple Silicon uses unified memory -- GPU VRAM = system RAM.
            // Don't double-count memory when both CPU and GPU resources are requested.
            // The node's `memory_total` already includes the GPU-accessible portion,
            // so we skip any separate GPU VRAM capacity check for Apple nodes.
            if node.resources.is_unified_memory() {
                debug!(
                    node = %node.id,
                    "Apple Silicon unified memory: GPU VRAM is part of system RAM, \
                     no separate VRAM budget check needed"
                );
            }
        }
    }

    // Check based on node mode
    match node_mode {
        NodeMode::Shared => {
            // In shared mode, check if node has capacity
            // For now, just check if node is healthy (resource checking can be added)
            true
        }
        NodeMode::Dedicated => {
            // In dedicated mode, check if node has no containers from THIS service
            // (allows containers from other services)
            !placements.has_service_on_node(node.id, service_name)
        }
        NodeMode::Exclusive => {
            // In exclusive mode, node must have NO containers at all
            !placements.has_any_containers(node.id)
        }
    }
}

/// Place replicas of a service according to its `node_mode`
///
/// # Arguments
/// * `service_name` - Name of the service
/// * `service_spec` - Service specification containing `node_mode` and `node_selector`
/// * `replicas` - Number of replicas to place
/// * `nodes` - Available nodes in the cluster
/// * `placements` - Current placement state (will be mutated)
///
/// # Returns
/// Vector of placement decisions for each replica
pub fn place_service_replicas(
    service_name: &str,
    service_spec: &ServiceSpec,
    replicas: u32,
    nodes: &mut [NodeState],
    placements: &mut PlacementState,
) -> Vec<PlacementDecision> {
    let mut decisions = Vec::with_capacity(replicas as usize);

    let gpu_count_requested = service_spec.resources.gpu.as_ref().map_or(0, |g| g.count);
    let gpu_sharing = service_spec.resources.gpu.as_ref().and_then(|g| g.sharing);

    for replica in 0..replicas {
        let container_id = ContainerId::new(service_name, replica);

        // Find suitable nodes
        let suitable_nodes: Vec<&NodeState> = nodes
            .iter()
            .filter(|n| {
                can_place_on_node(
                    n,
                    service_name,
                    service_spec.node_mode,
                    service_spec.node_selector.as_ref(),
                    placements,
                    Some(service_spec),
                )
            })
            .collect();

        if suitable_nodes.is_empty() {
            decisions.push(PlacementDecision {
                container_id,
                node_id: None,
                reason: PlacementReason::NoSuitableNode {
                    reason: format!(
                        "No node available for service '{}' with mode {:?}",
                        service_name, service_spec.node_mode
                    ),
                },
                gpu_indices: Vec::new(),
            });
            continue;
        }

        // Select the best node based on mode
        let selected = match service_spec.node_mode {
            NodeMode::Shared => {
                // Prefer nodes with lowest utilization (bin-packing)
                // Also consider preferred labels and GPU availability
                select_for_bin_packing(
                    &suitable_nodes,
                    service_spec.node_selector.as_ref(),
                    Some(service_spec),
                )
            }
            NodeMode::Dedicated | NodeMode::Exclusive => {
                // Prefer nodes with fewer existing containers
                select_for_isolation(
                    &suitable_nodes,
                    placements,
                    service_spec.node_selector.as_ref(),
                )
            }
        };

        let selected_id = selected.id;

        // Build the reason
        let reason = match service_spec.node_mode {
            NodeMode::Shared => PlacementReason::BinPacked {
                node_utilization: selected.utilization(),
            },
            NodeMode::Dedicated => PlacementReason::DedicatedNode,
            NodeMode::Exclusive => PlacementReason::ExclusiveNode,
        };

        // Record the placement
        placements.place(container_id.clone(), selected_id);

        // Allocate specific GPU indices on the selected node
        let gpu_indices = if gpu_count_requested > 0 {
            if let Some(node) = nodes.iter_mut().find(|n| n.id == selected_id) {
                node.resources
                    .allocate_gpus(gpu_count_requested, gpu_sharing)
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        decisions.push(PlacementDecision {
            container_id,
            node_id: Some(selected_id),
            reason,
            gpu_indices,
        });
    }

    // Gang scheduling: all-or-nothing. If any replica failed to place,
    // roll back all placements and return all-failed decisions.
    let is_gang = service_spec
        .resources
        .gpu
        .as_ref()
        .and_then(|g| g.scheduling)
        == Some(zlayer_spec::SchedulingPolicy::Gang);

    if is_gang && decisions.iter().any(|d| d.node_id.is_none()) {
        return gang_rollback(service_name, replicas, &decisions, nodes, placements);
    }

    decisions
}

/// Roll back a failed gang-scheduled placement.
///
/// Undoes all successful placements from `decisions`, frees GPU allocations,
/// and returns a vector of all-failed decisions.
#[allow(clippy::cast_possible_truncation)] // GPU count fits in u32
fn gang_rollback(
    service_name: &str,
    replicas: u32,
    decisions: &[PlacementDecision],
    nodes: &mut [NodeState],
    placements: &mut PlacementState,
) -> Vec<PlacementDecision> {
    let placed_count = decisions.iter().filter(|d| d.node_id.is_some()).count();
    debug!(
        service = service_name,
        replicas = replicas,
        placed = placed_count,
        "Gang scheduling failed: could not place all replicas, rolling back"
    );

    // Roll back: remove placements and free GPU allocations
    for decision in decisions {
        if let Some(node_id) = decision.node_id {
            placements.remove_service_from_node(node_id, service_name);
            if !decision.gpu_indices.is_empty() {
                if let Some(node) = nodes.iter_mut().find(|n| n.id == node_id) {
                    for &idx in &decision.gpu_indices {
                        if let Some(slot) = node.resources.gpu_allocated.get_mut(idx as usize) {
                            *slot = match slot {
                                GpuAllocation::Shared { current, max } if *current > 1 => {
                                    GpuAllocation::Shared {
                                        current: *current - 1,
                                        max: *max,
                                    }
                                }
                                GpuAllocation::Exclusive
                                | GpuAllocation::Shared { .. }
                                | GpuAllocation::Free => GpuAllocation::Free,
                            };
                        }
                    }
                    node.resources.gpu_used = node
                        .resources
                        .gpu_allocated
                        .iter()
                        .filter(|a| !a.is_free())
                        .count() as u32;
                }
            }
        }
    }

    // Return all-failed decisions
    (0..replicas)
        .map(|replica| PlacementDecision {
            container_id: ContainerId::new(service_name, replica),
            node_id: None,
            gpu_indices: Vec::new(),
            reason: PlacementReason::NoSuitableNode {
                reason: format!(
                    "Gang scheduling: could not place all {replicas} replicas of '{service_name}'"
                ),
            },
        })
        .collect()
}

/// Select a node for bin-packing (shared mode)
///
/// When the service requests GPUs, GPU availability is factored into scoring
/// with a 30% weight. Non-GPU workloads are scored purely on label preference
/// and CPU/memory utilization as before.
fn select_for_bin_packing<'a>(
    nodes: &[&'a NodeState],
    node_selector: Option<&NodeSelector>,
    service_spec: Option<&ServiceSpec>,
) -> &'a NodeState {
    let wants_gpu = service_spec
        .and_then(|s| s.resources.gpu.as_ref())
        .is_some();

    let wants_spread = service_spec
        .and_then(|s| s.resources.gpu.as_ref())
        .and_then(|g| g.scheduling)
        == Some(zlayer_spec::SchedulingPolicy::Spread);

    nodes
        .iter()
        .max_by(|a, b| {
            let a_pref = node_selector.map_or(0, |s| a.preferred_label_score(s));
            let b_pref = node_selector.map_or(0, |s| b.preferred_label_score(s));

            // First compare by preferred labels (more is better)
            match a_pref.cmp(&b_pref) {
                std::cmp::Ordering::Equal => {
                    if wants_gpu {
                        // For GPU workloads, compute a combined score:
                        // 70% weight for low utilization + 30% weight for GPU availability
                        let a_util_score = 100.0 - a.utilization(); // higher is better
                        let b_util_score = 100.0 - b.utilization();

                        let a_gpu_score = if a.resources.gpu_total > 0 {
                            let ratio = f64::from(a.resources.gpu_available())
                                / f64::from(a.resources.gpu_total);
                            if wants_spread {
                                // Spread: prefer nodes with FEWER available GPUs
                                (1.0 - ratio) * 100.0
                            } else {
                                // Pack: prefer nodes with MORE available GPUs
                                ratio * 100.0
                            }
                        } else {
                            0.0
                        };
                        let b_gpu_score = if b.resources.gpu_total > 0 {
                            let ratio = f64::from(b.resources.gpu_available())
                                / f64::from(b.resources.gpu_total);
                            if wants_spread {
                                // Spread: prefer nodes with FEWER available GPUs
                                (1.0 - ratio) * 100.0
                            } else {
                                // Pack: prefer nodes with MORE available GPUs
                                ratio * 100.0
                            }
                        } else {
                            0.0
                        };

                        let a_combined = a_util_score * 0.7 + a_gpu_score * 0.3;
                        let b_combined = b_util_score * 0.7 + b_gpu_score * 0.3;

                        a_combined
                            .partial_cmp(&b_combined)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    } else {
                        // Non-GPU: by utilization (lower is better for bin-packing)
                        // Note: we're using max_by, so we reverse the comparison
                        b.utilization()
                            .partial_cmp(&a.utilization())
                            .unwrap_or(std::cmp::Ordering::Equal)
                    }
                }
                other => other,
            }
        })
        .expect("nodes should not be empty")
}

/// Select a node for isolation (dedicated/exclusive mode)
fn select_for_isolation<'a>(
    nodes: &[&'a NodeState],
    placements: &PlacementState,
    node_selector: Option<&NodeSelector>,
) -> &'a NodeState {
    // Sort by: 1) preferred label score (descending), 2) container count (ascending)
    nodes
        .iter()
        .max_by(|a, b| {
            let a_pref = node_selector.map_or(0, |s| a.preferred_label_score(s));
            let b_pref = node_selector.map_or(0, |s| b.preferred_label_score(s));

            match a_pref.cmp(&b_pref) {
                std::cmp::Ordering::Equal => {
                    // Fewer containers is better
                    let a_count = placements.container_count(a.id);
                    let b_count = placements.container_count(b.id);
                    b_count.cmp(&a_count) // Reverse: we want fewer containers
                }
                other => other,
            }
        })
        .expect("nodes should not be empty")
}

/// Validate that there are enough nodes for services with dedicated/exclusive modes.
///
/// # Arguments
/// * `services` - Map of service name to (`node_mode`, replicas) pairs
/// * `available_nodes` - Number of available nodes in the cluster
///
/// # Errors
///
/// Returns `SchedulerError::InvalidConfig` if there are not enough nodes
/// to satisfy the placement requirements.
#[allow(clippy::implicit_hasher)]
pub fn validate_placement_feasibility(
    services: &HashMap<String, (NodeMode, u32)>,
    available_nodes: usize,
) -> Result<()> {
    let mut dedicated_replicas: usize = 0;
    let mut exclusive_services: Vec<(String, u32)> = Vec::new();

    for (service_name, (node_mode, replicas)) in services {
        match node_mode {
            NodeMode::Dedicated => {
                // Each dedicated replica needs a unique node
                dedicated_replicas += *replicas as usize;
            }
            NodeMode::Exclusive => {
                // Each exclusive service needs its own set of nodes
                exclusive_services.push((service_name.clone(), *replicas));
            }
            NodeMode::Shared => {
                // Shared services can be bin-packed
            }
        }
    }

    // Calculate total exclusive replicas
    let exclusive_replicas: usize = exclusive_services.iter().map(|(_, r)| *r as usize).sum();

    // Total nodes needed for dedicated + exclusive
    let required_nodes = dedicated_replicas + exclusive_replicas;

    if required_nodes > available_nodes {
        let mut details = Vec::new();
        if dedicated_replicas > 0 {
            details.push(format!("{dedicated_replicas} dedicated replicas"));
        }
        if !exclusive_services.is_empty() {
            let exclusive_detail: Vec<String> = exclusive_services
                .iter()
                .map(|(name, replicas)| format!("{name}({replicas})"))
                .collect();
            details.push(format!(
                "exclusive services: {}",
                exclusive_detail.join(", ")
            ));
        }

        return Err(SchedulerError::InvalidConfig(format!(
            "Insufficient nodes: need {} nodes for {}, but only {} available",
            required_nodes,
            details.join(", "),
            available_nodes
        )));
    }

    // Additional check: exclusive services cannot share nodes
    // This is implicitly handled by the exclusive mode logic, but we can validate here
    // that we have enough distinct nodes for all exclusive services
    if exclusive_services.len() > 1 {
        // Multiple exclusive services need completely separate node pools
        // For now, we just check total count - more sophisticated pool allocation
        // would be needed for production use
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zlayer_spec::{ImageSpec, PullPolicy};

    fn make_node(id: NodeId, address: &str) -> NodeState {
        NodeState::new(id, address)
    }

    fn make_service_spec(node_mode: NodeMode, node_selector: Option<NodeSelector>) -> ServiceSpec {
        ServiceSpec {
            rtype: zlayer_spec::ResourceType::Service,
            schedule: None,
            image: ImageSpec {
                name: "test:latest".to_string(),
                pull_policy: PullPolicy::IfNotPresent,
            },
            resources: zlayer_spec::ResourcesSpec::default(),
            env: HashMap::default(),
            command: zlayer_spec::CommandSpec::default(),
            network: zlayer_spec::ServiceNetworkSpec::default(),
            endpoints: vec![],
            scale: zlayer_spec::ScaleSpec::default(),
            depends: vec![],
            health: zlayer_spec::HealthSpec {
                start_grace: None,
                interval: None,
                timeout: None,
                retries: 3,
                check: zlayer_spec::HealthCheck::Tcp { port: 8080 },
            },
            init: zlayer_spec::InitSpec::default(),
            errors: zlayer_spec::ErrorsSpec::default(),
            devices: vec![],
            storage: vec![],
            capabilities: vec![],
            privileged: false,
            node_mode,
            node_selector,
            service_type: zlayer_spec::ServiceType::default(),
            wasm: None,
            logs: None,
            host_network: false,
        }
    }

    #[test]
    fn test_container_id_display() {
        let id = ContainerId::new("api", 2);
        assert_eq!(format!("{id}"), "api-2");
    }

    #[test]
    fn test_node_resources_utilization() {
        let mut res = NodeResources::new(4.0, 8 * 1024 * 1024 * 1024);
        assert!(res.utilization().abs() < f64::EPSILON);

        res.cpu_used = 2.0;
        res.memory_used = 4 * 1024 * 1024 * 1024;
        assert!((res.cpu_utilization() - 50.0).abs() < f64::EPSILON);
        assert!((res.memory_utilization() - 50.0).abs() < f64::EPSILON);
        assert!((res.utilization() - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_node_label_matching() {
        let node = NodeState::new(1, "192.168.1.1:8000")
            .with_label("gpu", "true")
            .with_label("zone", "us-east");

        let selector = NodeSelector {
            labels: [("gpu".to_string(), "true".to_string())]
                .into_iter()
                .collect(),
            prefer_labels: HashMap::new(),
        };

        assert!(node.matches_required_labels(&selector));

        let bad_selector = NodeSelector {
            labels: [("gpu".to_string(), "false".to_string())]
                .into_iter()
                .collect(),
            prefer_labels: HashMap::new(),
        };

        assert!(!node.matches_required_labels(&bad_selector));
    }

    #[test]
    fn test_preferred_label_score() {
        let node = NodeState::new(1, "192.168.1.1:8000")
            .with_label("storage", "ssd")
            .with_label("zone", "us-east");

        let selector = NodeSelector {
            labels: HashMap::new(),
            prefer_labels: [
                ("storage".to_string(), "ssd".to_string()),
                ("zone".to_string(), "us-east".to_string()),
                ("rack".to_string(), "a1".to_string()), // Not present on node
            ]
            .into_iter()
            .collect(),
        };

        assert_eq!(node.preferred_label_score(&selector), 2);
    }

    #[test]
    fn test_placement_state_basic() {
        let mut state = PlacementState::new();

        let container = ContainerId::new("api", 0);
        state.place(container.clone(), 1);

        assert!(state.has_service_on_node(1, "api"));
        assert!(!state.has_service_on_node(2, "api"));
        assert_eq!(state.node_for_container(&container), Some(1));
        assert_eq!(state.container_count(1), 1);
    }

    #[test]
    fn test_placement_state_remove_node() {
        let mut state = PlacementState::new();

        state.place(ContainerId::new("api", 0), 1);
        state.place(ContainerId::new("api", 1), 1);
        state.place(ContainerId::new("web", 0), 1);
        state.place(ContainerId::new("web", 1), 2);

        assert_eq!(state.container_count(1), 3);
        assert_eq!(state.container_count(2), 1);

        let removed = state.remove_node(1);
        assert_eq!(removed.len(), 3);
        assert_eq!(state.container_count(1), 0);
        assert!(!state.has_service_on_node(1, "api"));
        assert!(!state.has_service_on_node(1, "web"));
        // Node 2 should be unaffected
        assert_eq!(state.container_count(2), 1);
        assert!(state.has_service_on_node(2, "web"));
    }

    #[test]
    fn test_placement_state_remove_service_from_node() {
        let mut state = PlacementState::new();

        state.place(ContainerId::new("api", 0), 1);
        state.place(ContainerId::new("api", 1), 1);
        state.place(ContainerId::new("web", 0), 1);

        assert_eq!(state.container_count(1), 3);

        let removed = state.remove_service_from_node(1, "api");
        assert_eq!(removed.len(), 2);
        assert!(!state.has_service_on_node(1, "api"));
        // web should still be on node 1
        assert!(state.has_service_on_node(1, "web"));
        assert_eq!(state.container_count(1), 1);
    }

    #[test]
    fn test_placement_state_remove_service_from_empty_node() {
        let mut state = PlacementState::new();
        let removed = state.remove_service_from_node(99, "api");
        assert!(removed.is_empty());
    }

    #[test]
    fn test_can_place_shared_mode() {
        let node = make_node(1, "192.168.1.1:8000");
        let mut placements = PlacementState::new();

        // Place some containers
        placements.place(ContainerId::new("other", 0), 1);
        placements.place(ContainerId::new("other", 1), 1);

        // Shared mode should still allow placement
        assert!(can_place_on_node(
            &node,
            "api",
            NodeMode::Shared,
            None,
            &placements,
            None,
        ));
    }

    #[test]
    fn test_can_place_dedicated_mode() {
        let node = make_node(1, "192.168.1.1:8000");
        let mut placements = PlacementState::new();

        // Initially can place
        assert!(can_place_on_node(
            &node,
            "api",
            NodeMode::Dedicated,
            None,
            &placements,
            None,
        ));

        // Place a container from another service - still can place
        placements.place(ContainerId::new("other", 0), 1);
        assert!(can_place_on_node(
            &node,
            "api",
            NodeMode::Dedicated,
            None,
            &placements,
            None,
        ));

        // Place a container from same service - cannot place another
        placements.place(ContainerId::new("api", 0), 1);
        assert!(!can_place_on_node(
            &node,
            "api",
            NodeMode::Dedicated,
            None,
            &placements,
            None,
        ));
    }

    #[test]
    fn test_can_place_exclusive_mode() {
        let node = make_node(1, "192.168.1.1:8000");
        let mut placements = PlacementState::new();

        // Initially can place
        assert!(can_place_on_node(
            &node,
            "db",
            NodeMode::Exclusive,
            None,
            &placements,
            None,
        ));

        // Place any container - cannot place exclusive anymore
        placements.place(ContainerId::new("other", 0), 1);
        assert!(!can_place_on_node(
            &node,
            "db",
            NodeMode::Exclusive,
            None,
            &placements,
            None,
        ));
    }

    #[test]
    fn test_place_service_replicas_shared() {
        let mut nodes = vec![
            make_node(1, "192.168.1.1:8000"),
            make_node(2, "192.168.1.2:8000"),
        ];
        let mut placements = PlacementState::new();
        let spec = make_service_spec(NodeMode::Shared, None);

        let decisions = place_service_replicas("api", &spec, 3, &mut nodes, &mut placements);

        assert_eq!(decisions.len(), 3);
        assert!(decisions.iter().all(PlacementDecision::is_success));
    }

    #[test]
    fn test_place_service_replicas_dedicated() {
        let mut nodes = vec![
            make_node(1, "192.168.1.1:8000"),
            make_node(2, "192.168.1.2:8000"),
            make_node(3, "192.168.1.3:8000"),
        ];
        let mut placements = PlacementState::new();
        let spec = make_service_spec(NodeMode::Dedicated, None);

        let decisions = place_service_replicas("api", &spec, 3, &mut nodes, &mut placements);

        assert_eq!(decisions.len(), 3);
        assert!(decisions.iter().all(PlacementDecision::is_success));

        // Each replica should be on a different node
        let assigned_nodes: Vec<NodeId> = decisions.iter().filter_map(|d| d.node_id).collect();
        assert_eq!(assigned_nodes.len(), 3);
        let unique_nodes: std::collections::HashSet<_> = assigned_nodes.iter().collect();
        assert_eq!(unique_nodes.len(), 3);
    }

    #[test]
    fn test_place_service_replicas_dedicated_insufficient_nodes() {
        let mut nodes = vec![
            make_node(1, "192.168.1.1:8000"),
            make_node(2, "192.168.1.2:8000"),
        ];
        let mut placements = PlacementState::new();
        let spec = make_service_spec(NodeMode::Dedicated, None);

        let decisions = place_service_replicas("api", &spec, 3, &mut nodes, &mut placements);

        assert_eq!(decisions.len(), 3);
        // First 2 should succeed, 3rd should fail
        assert!(decisions[0].is_success());
        assert!(decisions[1].is_success());
        assert!(!decisions[2].is_success());
        assert!(matches!(
            decisions[2].reason,
            PlacementReason::NoSuitableNode { .. }
        ));
    }

    #[test]
    fn test_place_service_replicas_exclusive() {
        let mut nodes = vec![
            make_node(1, "192.168.1.1:8000"),
            make_node(2, "192.168.1.2:8000"),
        ];
        let mut placements = PlacementState::new();
        let spec = make_service_spec(NodeMode::Exclusive, None);

        // Place 2 replicas - should use both nodes
        let decisions = place_service_replicas("db", &spec, 2, &mut nodes, &mut placements);

        assert_eq!(decisions.len(), 2);
        assert!(decisions.iter().all(PlacementDecision::is_success));

        // Now try to place another service - should fail (nodes are exclusive)
        let spec2 = make_service_spec(NodeMode::Exclusive, None);
        let decisions2 = place_service_replicas("cache", &spec2, 1, &mut nodes, &mut placements);

        assert!(!decisions2[0].is_success());
    }

    #[test]
    fn test_place_with_node_selector() {
        let mut nodes = vec![
            make_node(1, "192.168.1.1:8000").with_label("gpu", "true"),
            make_node(2, "192.168.1.2:8000").with_label("gpu", "false"),
            make_node(3, "192.168.1.3:8000").with_label("gpu", "true"),
        ];
        let mut placements = PlacementState::new();

        let selector = NodeSelector {
            labels: [("gpu".to_string(), "true".to_string())]
                .into_iter()
                .collect(),
            prefer_labels: HashMap::new(),
        };
        let spec = make_service_spec(NodeMode::Shared, Some(selector));

        let decisions = place_service_replicas("ml", &spec, 2, &mut nodes, &mut placements);

        assert_eq!(decisions.len(), 2);
        assert!(decisions.iter().all(PlacementDecision::is_success));

        // All placements should be on GPU nodes (1 or 3)
        let assigned_nodes: Vec<NodeId> = decisions.iter().filter_map(|d| d.node_id).collect();
        for node_id in assigned_nodes {
            assert!(node_id == 1 || node_id == 3);
        }
    }

    #[test]
    fn test_validate_placement_feasibility_success() {
        let services = HashMap::from([
            ("api".to_string(), (NodeMode::Dedicated, 3)),
            ("cache".to_string(), (NodeMode::Shared, 5)),
            ("db".to_string(), (NodeMode::Exclusive, 1)),
        ]);

        // Need 3 (dedicated) + 1 (exclusive) = 4 nodes
        assert!(validate_placement_feasibility(&services, 5).is_ok());
        assert!(validate_placement_feasibility(&services, 4).is_ok());
    }

    #[test]
    fn test_validate_placement_feasibility_insufficient() {
        let services = HashMap::from([
            ("api".to_string(), (NodeMode::Dedicated, 3)),
            ("db".to_string(), (NodeMode::Exclusive, 2)),
        ]);

        // Need 3 + 2 = 5 nodes
        let result = validate_placement_feasibility(&services, 4);
        assert!(result.is_err());

        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Insufficient nodes"));
    }

    #[test]
    fn test_validate_placement_shared_only() {
        let services = HashMap::from([
            ("api".to_string(), (NodeMode::Shared, 100)),
            ("cache".to_string(), (NodeMode::Shared, 50)),
        ]);

        // Shared services can all fit on 1 node (theoretically)
        assert!(validate_placement_feasibility(&services, 1).is_ok());
    }

    #[test]
    fn test_unhealthy_node_excluded() {
        let mut node = make_node(1, "192.168.1.1:8000");
        node.healthy = false;

        let placements = PlacementState::new();
        assert!(!can_place_on_node(
            &node,
            "api",
            NodeMode::Shared,
            None,
            &placements,
            None,
        ));
    }

    // ==========================================================================
    // GPU-aware scheduling tests
    // ==========================================================================

    /// Helper to create a node with GPU resources
    fn make_gpu_node(id: NodeId, address: &str, gpu_total: u32, vendor: &str) -> NodeState {
        let mut resources = NodeResources::new(8.0, 32 * 1024 * 1024 * 1024);
        resources.gpu_total = gpu_total;
        resources.gpu_vendor = vendor.to_string();
        resources.gpu_models = vec!["Test GPU".to_string(); gpu_total as usize];
        resources.gpu_memory_mb = u64::from(gpu_total) * 16384; // 16GB per GPU
        resources.gpu_allocated = vec![GpuAllocation::Free; gpu_total as usize];
        NodeState::new(id, address).with_resources(resources)
    }

    /// Helper to create a service spec that requests GPUs
    fn make_gpu_service_spec(gpu_count: u32, gpu_vendor: &str, node_mode: NodeMode) -> ServiceSpec {
        let mut spec = make_service_spec(node_mode, None);
        spec.resources.gpu = Some(zlayer_spec::GpuSpec {
            count: gpu_count,
            vendor: gpu_vendor.to_string(),
            mode: None,
            model: None,
            scheduling: None,
            distributed: None,
            sharing: None,
        });
        spec
    }

    #[test]
    fn test_gpu_service_rejected_on_node_without_gpus() {
        let node = make_node(1, "192.168.1.1:8000"); // No GPUs
        let placements = PlacementState::new();
        let spec = make_gpu_service_spec(1, "nvidia", NodeMode::Shared);

        assert!(!can_place_on_node(
            &node,
            "ml-training",
            NodeMode::Shared,
            None,
            &placements,
            Some(&spec),
        ));
    }

    #[test]
    fn test_gpu_service_placed_on_node_with_sufficient_gpus() {
        let node = make_gpu_node(1, "192.168.1.1:8000", 4, "nvidia");
        let placements = PlacementState::new();
        let spec = make_gpu_service_spec(2, "nvidia", NodeMode::Shared);

        assert!(can_place_on_node(
            &node,
            "ml-training",
            NodeMode::Shared,
            None,
            &placements,
            Some(&spec),
        ));
    }

    #[test]
    fn test_gpu_service_rejected_insufficient_available_gpus() {
        let mut node = make_gpu_node(1, "192.168.1.1:8000", 4, "nvidia");
        node.resources.gpu_used = 3; // Only 1 GPU available
        node.resources.gpu_allocated = vec![
            GpuAllocation::Exclusive,
            GpuAllocation::Exclusive,
            GpuAllocation::Exclusive,
            GpuAllocation::Free,
        ];
        let placements = PlacementState::new();
        let spec = make_gpu_service_spec(2, "nvidia", NodeMode::Shared);

        assert!(!can_place_on_node(
            &node,
            "ml-training",
            NodeMode::Shared,
            None,
            &placements,
            Some(&spec),
        ));
    }

    #[test]
    fn test_gpu_vendor_mismatch_rejected() {
        let node = make_gpu_node(1, "192.168.1.1:8000", 4, "amd");
        let placements = PlacementState::new();
        let spec = make_gpu_service_spec(1, "nvidia", NodeMode::Shared);

        assert!(!can_place_on_node(
            &node,
            "ml-training",
            NodeMode::Shared,
            None,
            &placements,
            Some(&spec),
        ));
    }

    #[test]
    fn test_gpu_vendor_empty_matches_any() {
        // If the service has an empty vendor, it should match any GPU node
        let node = make_gpu_node(1, "192.168.1.1:8000", 4, "nvidia");
        let placements = PlacementState::new();
        let spec = make_gpu_service_spec(1, "", NodeMode::Shared);

        assert!(can_place_on_node(
            &node,
            "ml-training",
            NodeMode::Shared,
            None,
            &placements,
            Some(&spec),
        ));
    }

    #[test]
    fn test_non_gpu_service_ignores_gpu_fields() {
        // A service without GPU requirements should be placeable on any node,
        // regardless of the node's GPU state
        let node_no_gpu = make_node(1, "192.168.1.1:8000");
        let node_with_gpu = make_gpu_node(2, "192.168.1.2:8000", 4, "nvidia");
        let placements = PlacementState::new();
        let spec = make_service_spec(NodeMode::Shared, None); // No GPU requirement

        assert!(can_place_on_node(
            &node_no_gpu,
            "api",
            NodeMode::Shared,
            None,
            &placements,
            Some(&spec),
        ));
        assert!(can_place_on_node(
            &node_with_gpu,
            "api",
            NodeMode::Shared,
            None,
            &placements,
            Some(&spec),
        ));
    }

    #[test]
    fn test_gpu_placement_tracks_usage() {
        let mut nodes = vec![
            make_gpu_node(1, "192.168.1.1:8000", 2, "nvidia"),
            make_gpu_node(2, "192.168.1.2:8000", 2, "nvidia"),
        ];
        let mut placements = PlacementState::new();
        let spec = make_gpu_service_spec(2, "nvidia", NodeMode::Shared);

        // Place first replica -- should succeed and consume 2 GPUs on one node
        let decisions = place_service_replicas("ml", &spec, 1, &mut nodes, &mut placements);
        assert_eq!(decisions.len(), 1);
        assert!(decisions[0].is_success());
        let first_node = decisions[0].node_id.unwrap();

        // The placed node should now have 2 GPUs used
        let placed_node = nodes.iter().find(|n| n.id == first_node).unwrap();
        assert_eq!(placed_node.resources.gpu_used, 2);
        assert_eq!(placed_node.resources.gpu_available(), 0);

        // Place second replica -- should go to the OTHER node since first is full
        let decisions2 = place_service_replicas("ml2", &spec, 1, &mut nodes, &mut placements);
        assert_eq!(decisions2.len(), 1);
        assert!(decisions2[0].is_success());
        let second_node = decisions2[0].node_id.unwrap();
        assert_ne!(first_node, second_node);
    }

    #[test]
    fn test_gpu_placement_exhaustion() {
        let mut nodes = vec![make_gpu_node(1, "192.168.1.1:8000", 2, "nvidia")];
        let mut placements = PlacementState::new();
        let spec = make_gpu_service_spec(2, "nvidia", NodeMode::Shared);

        // First placement should succeed
        let decisions = place_service_replicas("ml1", &spec, 1, &mut nodes, &mut placements);
        assert!(decisions[0].is_success());

        // Second placement should fail - no more GPUs available
        let decisions2 = place_service_replicas("ml2", &spec, 1, &mut nodes, &mut placements);
        assert!(!decisions2[0].is_success());
        assert!(matches!(
            decisions2[0].reason,
            PlacementReason::NoSuitableNode { .. }
        ));
    }

    #[test]
    fn test_gpu_scoring_prefers_more_available_gpus() {
        // Node 1: 4 GPUs, 2 used (2 available)
        // Node 2: 4 GPUs, 0 used (4 available)
        // Both have equal CPU/memory utilization
        let mut gpu_node_a = make_gpu_node(1, "192.168.1.1:8000", 4, "nvidia");
        gpu_node_a.resources.gpu_used = 2;
        gpu_node_a.resources.gpu_allocated = vec![
            GpuAllocation::Exclusive,
            GpuAllocation::Exclusive,
            GpuAllocation::Free,
            GpuAllocation::Free,
        ];
        let gpu_node_b = make_gpu_node(2, "192.168.1.2:8000", 4, "nvidia");

        let mut nodes = vec![gpu_node_a, gpu_node_b];
        let mut placements = PlacementState::new();
        let spec = make_gpu_service_spec(1, "nvidia", NodeMode::Shared);

        let decisions = place_service_replicas("ml", &spec, 1, &mut nodes, &mut placements);
        assert!(decisions[0].is_success());

        // Should prefer node 2 (more GPUs available)
        assert_eq!(decisions[0].node_id, Some(2));
    }

    // ==========================================================================
    // Apple Silicon unified memory tests
    // ==========================================================================

    #[test]
    fn test_apple_silicon_is_unified_memory() {
        let apple_node = make_gpu_node(1, "192.168.1.1:8000", 1, "apple");
        assert!(apple_node.resources.is_unified_memory());

        let nvidia_node = make_gpu_node(2, "192.168.1.2:8000", 4, "nvidia");
        assert!(!nvidia_node.resources.is_unified_memory());

        let no_gpu_node = make_node(3, "192.168.1.3:8000");
        assert!(!no_gpu_node.resources.is_unified_memory());
    }

    #[test]
    fn test_unified_memory_no_double_count() {
        // Apple Silicon: 32 GB system RAM, GPU "VRAM" is 24 GB of that same RAM
        let mut apple_res = NodeResources::new(10.0, 32 * 1024 * 1024 * 1024);
        apple_res.gpu_total = 1;
        apple_res.gpu_vendor = "apple".to_string();
        apple_res.gpu_memory_mb = 24576; // 24 GB "VRAM" (subset of system RAM)

        // Effective memory should be system RAM only (32 GB = 32768 MB)
        assert_eq!(apple_res.total_effective_memory_mb(), 32768);

        // Discrete GPU: 32 GB system RAM + 16 GB discrete VRAM
        let mut nvidia_res = NodeResources::new(8.0, 32 * 1024 * 1024 * 1024);
        nvidia_res.gpu_total = 1;
        nvidia_res.gpu_vendor = "nvidia".to_string();
        nvidia_res.gpu_memory_mb = 16384; // 16 GB discrete VRAM

        // Effective memory should be system RAM + VRAM (32768 + 16384 = 49152 MB)
        assert_eq!(nvidia_res.total_effective_memory_mb(), 49152);
    }

    #[test]
    fn test_apple_gpu_service_can_be_placed() {
        let apple_node = make_gpu_node(1, "192.168.1.1:8000", 1, "apple");
        let placements = PlacementState::new();
        let spec = make_gpu_service_spec(1, "apple", NodeMode::Shared);

        assert!(can_place_on_node(
            &apple_node,
            "ml-inference",
            NodeMode::Shared,
            None,
            &placements,
            Some(&spec),
        ));
    }

    // ==========================================================================
    // Gang scheduling tests
    // ==========================================================================

    #[test]
    fn test_gang_scheduling_rolls_back_on_partial_failure() {
        // 2 nodes with 1 GPU each, service requests 1 GPU per replica with 3 replicas
        let mut nodes = vec![
            make_gpu_node(1, "192.168.1.1:8000", 1, "nvidia"),
            make_gpu_node(2, "192.168.1.2:8000", 1, "nvidia"),
        ];
        let mut placements = PlacementState::new();

        let mut spec = make_service_spec(NodeMode::Shared, None);
        spec.resources.gpu = Some(zlayer_spec::GpuSpec {
            count: 1,
            vendor: "nvidia".to_string(),
            mode: None,
            model: None,
            scheduling: Some(zlayer_spec::SchedulingPolicy::Gang),
            distributed: None,
            sharing: None,
        });

        let decisions = place_service_replicas("gpu-gang", &spec, 3, &mut nodes, &mut placements);

        // All 3 should fail (only 2 nodes with GPUs)
        assert!(
            decisions.iter().all(|d| d.node_id.is_none()),
            "Gang scheduling should fail all when not all can be placed"
        );
        assert_eq!(decisions.len(), 3);

        // GPUs should be freed (rolled back)
        assert_eq!(nodes[0].resources.gpu_available(), 1);
        assert_eq!(nodes[1].resources.gpu_available(), 1);
    }

    #[test]
    fn test_gang_scheduling_succeeds_when_all_fit() {
        let mut nodes = vec![
            make_gpu_node(1, "192.168.1.1:8000", 2, "nvidia"),
            make_gpu_node(2, "192.168.1.2:8000", 2, "nvidia"),
        ];
        let mut placements = PlacementState::new();

        let mut spec = make_service_spec(NodeMode::Shared, None);
        spec.resources.gpu = Some(zlayer_spec::GpuSpec {
            count: 1,
            vendor: "nvidia".to_string(),
            mode: None,
            model: None,
            scheduling: Some(zlayer_spec::SchedulingPolicy::Gang),
            distributed: None,
            sharing: None,
        });

        let decisions = place_service_replicas("gpu-gang", &spec, 4, &mut nodes, &mut placements);

        // All 4 should succeed (2 nodes x 2 GPUs each)
        assert!(
            decisions.iter().all(|d| d.node_id.is_some()),
            "Gang scheduling should succeed when all fit"
        );
        assert_eq!(decisions.len(), 4);
    }

    // ==========================================================================
    // GPU spread scheduling tests
    // ==========================================================================

    #[test]
    fn test_spread_scheduling_distributes_across_nodes() {
        // fresh_node has 4 GPUs (all free), busy_node has 4 GPUs (2 used)
        // Spread should prefer busy_node (less GPU availability = more spread)
        let fresh_node = make_gpu_node(1, "192.168.1.1:8000", 4, "nvidia");
        let mut busy_node = make_gpu_node(2, "192.168.1.2:8000", 4, "nvidia");
        // Allocate 2 GPUs on busy_node
        busy_node.resources.gpu_allocated[0] = GpuAllocation::Exclusive;
        busy_node.resources.gpu_allocated[1] = GpuAllocation::Exclusive;
        busy_node.resources.gpu_used = 2;

        let mut nodes = vec![fresh_node, busy_node];
        let mut placements = PlacementState::new();

        let mut spec = make_service_spec(NodeMode::Shared, None);
        spec.resources.gpu = Some(zlayer_spec::GpuSpec {
            count: 1,
            vendor: "nvidia".to_string(),
            mode: None,
            model: None,
            scheduling: Some(zlayer_spec::SchedulingPolicy::Spread),
            distributed: None,
            sharing: None,
        });

        let decisions = place_service_replicas("spread-svc", &spec, 1, &mut nodes, &mut placements);

        // Should prefer node2 (less GPU availability) to spread the workload
        assert_eq!(decisions[0].node_id, Some(2));
    }
}
