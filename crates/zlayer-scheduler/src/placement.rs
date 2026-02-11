//! Node placement logic for ZLayer scheduler
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
use zlayer_spec::{NodeMode, NodeSelector, ServiceSpec};

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
    /// GPU model names (e.g., ["NVIDIA A100-SXM4-80GB"])
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
}

impl NodeResources {
    /// Create new node resources
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
        }
    }

    /// Get available CPU cores
    pub fn cpu_available(&self) -> f64 {
        (self.cpu_total - self.cpu_used).max(0.0)
    }

    /// Get available memory in bytes
    pub fn memory_available(&self) -> u64 {
        self.memory_total.saturating_sub(self.memory_used)
    }

    /// Calculate CPU utilization as a percentage
    pub fn cpu_utilization(&self) -> f64 {
        if self.cpu_total > 0.0 {
            (self.cpu_used / self.cpu_total) * 100.0
        } else {
            0.0
        }
    }

    /// Calculate memory utilization as a percentage
    pub fn memory_utilization(&self) -> f64 {
        if self.memory_total > 0 {
            (self.memory_used as f64 / self.memory_total as f64) * 100.0
        } else {
            0.0
        }
    }

    /// Calculate overall utilization (average of CPU and memory)
    pub fn utilization(&self) -> f64 {
        (self.cpu_utilization() + self.memory_utilization()) / 2.0
    }

    /// Get number of GPUs available for allocation
    pub fn gpu_available(&self) -> u32 {
        self.gpu_total.saturating_sub(self.gpu_used)
    }

    /// Returns `true` if this node uses unified memory (Apple Silicon).
    ///
    /// On Apple Silicon, GPU VRAM and system RAM are the same physical memory pool.
    /// This means `gpu_memory_mb` is NOT additive with `memory_total` -- they overlap.
    /// Callers must not double-count memory when both CPU and GPU resources are requested
    /// on a unified-memory node.
    pub fn is_unified_memory(&self) -> bool {
        self.gpu_vendor.eq_ignore_ascii_case("apple")
    }

    /// Returns the total effective memory in MB, correctly handling unified memory.
    ///
    /// - **Discrete GPU nodes** (NVIDIA, AMD, Intel): system RAM + GPU VRAM
    /// - **Unified memory nodes** (Apple Silicon): system RAM only (GPU VRAM is a subset)
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
    pub fn with_label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.labels.insert(key.into(), value.into());
        self
    }

    /// Set resources for the node
    pub fn with_resources(mut self, resources: NodeResources) -> Self {
        self.resources = resources;
        self
    }

    /// Get overall utilization percentage
    pub fn utilization(&self) -> f64 {
        self.resources.utilization()
    }

    /// Check if this node matches the required labels in a NodeSelector
    pub fn matches_required_labels(&self, selector: &NodeSelector) -> bool {
        selector
            .labels
            .iter()
            .all(|(key, value)| self.labels.get(key) == Some(value))
    }

    /// Count how many preferred labels this node matches
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
}

impl PlacementDecision {
    /// Check if placement was successful
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
    pub fn containers_on_node(&self, node_id: NodeId) -> &[ContainerId] {
        self.node_containers
            .get(&node_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Get the node for a specific container
    pub fn node_for_container(&self, container_id: &ContainerId) -> Option<NodeId> {
        self.container_nodes.get(container_id).copied()
    }

    /// Check if a node has any containers from a specific service
    pub fn has_service_on_node(&self, node_id: NodeId, service_name: &str) -> bool {
        self.containers_on_node(node_id)
            .iter()
            .any(|c| c.service == service_name)
    }

    /// Check if a node has any containers at all
    pub fn has_any_containers(&self, node_id: NodeId) -> bool {
        !self.containers_on_node(node_id).is_empty()
    }

    /// Get the count of containers on a node
    pub fn container_count(&self, node_id: NodeId) -> usize {
        self.containers_on_node(node_id).len()
    }
}

/// Check if a node can accept a service based on node_mode, constraints, and resource availability
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

/// Place replicas of a service according to its node_mode
///
/// # Arguments
/// * `service_name` - Name of the service
/// * `service_spec` - Service specification containing node_mode and node_selector
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

    let gpu_count_requested = service_spec
        .resources
        .gpu
        .as_ref()
        .map(|g| g.count)
        .unwrap_or(0);

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

        // Track GPU usage on the selected node so subsequent placements
        // see the reduced availability
        if gpu_count_requested > 0 {
            if let Some(node) = nodes.iter_mut().find(|n| n.id == selected_id) {
                node.resources.gpu_used =
                    node.resources.gpu_used.saturating_add(gpu_count_requested);
            }
        }

        decisions.push(PlacementDecision {
            container_id,
            node_id: Some(selected_id),
            reason,
        });
    }

    decisions
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
                            (a.resources.gpu_available() as f64 / a.resources.gpu_total as f64)
                                * 100.0
                        } else {
                            0.0
                        };
                        let b_gpu_score = if b.resources.gpu_total > 0 {
                            (b.resources.gpu_available() as f64 / b.resources.gpu_total as f64)
                                * 100.0
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

/// Validate that there are enough nodes for services with dedicated/exclusive modes
///
/// # Arguments
/// * `services` - Map of service name to (node_mode, replicas) pairs
/// * `available_nodes` - Number of available nodes in the cluster
///
/// # Returns
/// `Ok(())` if placement is feasible, or an error describing why it's not
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
            details.push(format!("{} dedicated replicas", dedicated_replicas));
        }
        if !exclusive_services.is_empty() {
            let exclusive_detail: Vec<String> = exclusive_services
                .iter()
                .map(|(name, replicas)| format!("{}({})", name, replicas))
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
            resources: Default::default(),
            env: Default::default(),
            command: Default::default(),
            network: Default::default(),
            endpoints: vec![],
            scale: Default::default(),
            depends: vec![],
            health: zlayer_spec::HealthSpec {
                start_grace: None,
                interval: None,
                timeout: None,
                retries: 3,
                check: zlayer_spec::HealthCheck::Tcp { port: 8080 },
            },
            init: Default::default(),
            errors: Default::default(),
            devices: vec![],
            storage: vec![],
            capabilities: vec![],
            privileged: false,
            node_mode,
            node_selector,
            service_type: Default::default(),
            wasm_http: None,
            host_network: false,
        }
    }

    #[test]
    fn test_container_id_display() {
        let id = ContainerId::new("api", 2);
        assert_eq!(format!("{}", id), "api-2");
    }

    #[test]
    fn test_node_resources_utilization() {
        let mut res = NodeResources::new(4.0, 8 * 1024 * 1024 * 1024);
        assert_eq!(res.utilization(), 0.0);

        res.cpu_used = 2.0;
        res.memory_used = 4 * 1024 * 1024 * 1024;
        assert_eq!(res.cpu_utilization(), 50.0);
        assert_eq!(res.memory_utilization(), 50.0);
        assert_eq!(res.utilization(), 50.0);
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
        assert!(decisions.iter().all(|d| d.is_success()));
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
        assert!(decisions.iter().all(|d| d.is_success()));

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
        assert!(decisions.iter().all(|d| d.is_success()));

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
        assert!(decisions.iter().all(|d| d.is_success()));

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
        resources.gpu_memory_mb = gpu_total as u64 * 16384; // 16GB per GPU
        NodeState::new(id, address).with_resources(resources)
    }

    /// Helper to create a service spec that requests GPUs
    fn make_gpu_service_spec(gpu_count: u32, gpu_vendor: &str, node_mode: NodeMode) -> ServiceSpec {
        let mut spec = make_service_spec(node_mode, None);
        spec.resources.gpu = Some(zlayer_spec::GpuSpec {
            count: gpu_count,
            vendor: gpu_vendor.to_string(),
            mode: None,
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
        let mut node1 = make_gpu_node(1, "192.168.1.1:8000", 4, "nvidia");
        node1.resources.gpu_used = 2;
        let node2 = make_gpu_node(2, "192.168.1.2:8000", 4, "nvidia");

        let mut nodes = vec![node1, node2];
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
}
