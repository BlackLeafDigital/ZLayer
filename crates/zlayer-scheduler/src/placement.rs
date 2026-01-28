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
}

impl NodeResources {
    /// Create new node resources
    pub fn new(cpu_total: f64, memory_total: u64) -> Self {
        Self {
            cpu_total,
            cpu_used: 0.0,
            memory_total,
            memory_used: 0,
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

/// Check if a node can accept a service based on node_mode and constraints
///
/// # Arguments
/// * `node` - The node to check
/// * `service_name` - Name of the service being placed
/// * `node_mode` - The node allocation mode for the service
/// * `node_selector` - Optional node selection constraints
/// * `placements` - Current placement state
///
/// # Returns
/// `true` if the node can accept a replica of the service
pub fn can_place_on_node(
    node: &NodeState,
    service_name: &str,
    node_mode: NodeMode,
    node_selector: Option<&NodeSelector>,
    placements: &PlacementState,
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
    nodes: &[NodeState],
    placements: &mut PlacementState,
) -> Vec<PlacementDecision> {
    let mut decisions = Vec::with_capacity(replicas as usize);

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
                // Also consider preferred labels if node_selector is present
                select_for_bin_packing(&suitable_nodes, service_spec.node_selector.as_ref())
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

        // Build the reason
        let reason = match service_spec.node_mode {
            NodeMode::Shared => PlacementReason::BinPacked {
                node_utilization: selected.utilization(),
            },
            NodeMode::Dedicated => PlacementReason::DedicatedNode,
            NodeMode::Exclusive => PlacementReason::ExclusiveNode,
        };

        // Record the placement
        placements.place(container_id.clone(), selected.id);

        decisions.push(PlacementDecision {
            container_id,
            node_id: Some(selected.id),
            reason,
        });
    }

    decisions
}

/// Select a node for bin-packing (shared mode)
fn select_for_bin_packing<'a>(
    nodes: &[&'a NodeState],
    node_selector: Option<&NodeSelector>,
) -> &'a NodeState {
    // Sort by: 1) preferred label score (descending), 2) utilization (ascending)
    nodes
        .iter()
        .max_by(|a, b| {
            let a_pref = node_selector.map_or(0, |s| a.preferred_label_score(s));
            let b_pref = node_selector.map_or(0, |s| b.preferred_label_score(s));

            // First compare by preferred labels (more is better)
            match a_pref.cmp(&b_pref) {
                std::cmp::Ordering::Equal => {
                    // Then by utilization (lower is better for bin-packing)
                    // Note: we're using max_by, so we reverse the comparison
                    b.utilization()
                        .partial_cmp(&a.utilization())
                        .unwrap_or(std::cmp::Ordering::Equal)
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
            &placements
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
            &placements
        ));

        // Place a container from another service - still can place
        placements.place(ContainerId::new("other", 0), 1);
        assert!(can_place_on_node(
            &node,
            "api",
            NodeMode::Dedicated,
            None,
            &placements
        ));

        // Place a container from same service - cannot place another
        placements.place(ContainerId::new("api", 0), 1);
        assert!(!can_place_on_node(
            &node,
            "api",
            NodeMode::Dedicated,
            None,
            &placements
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
            &placements
        ));

        // Place any container - cannot place exclusive anymore
        placements.place(ContainerId::new("other", 0), 1);
        assert!(!can_place_on_node(
            &node,
            "db",
            NodeMode::Exclusive,
            None,
            &placements
        ));
    }

    #[test]
    fn test_place_service_replicas_shared() {
        let nodes = vec![
            make_node(1, "192.168.1.1:8000"),
            make_node(2, "192.168.1.2:8000"),
        ];
        let mut placements = PlacementState::new();
        let spec = make_service_spec(NodeMode::Shared, None);

        let decisions = place_service_replicas("api", &spec, 3, &nodes, &mut placements);

        assert_eq!(decisions.len(), 3);
        assert!(decisions.iter().all(|d| d.is_success()));
    }

    #[test]
    fn test_place_service_replicas_dedicated() {
        let nodes = vec![
            make_node(1, "192.168.1.1:8000"),
            make_node(2, "192.168.1.2:8000"),
            make_node(3, "192.168.1.3:8000"),
        ];
        let mut placements = PlacementState::new();
        let spec = make_service_spec(NodeMode::Dedicated, None);

        let decisions = place_service_replicas("api", &spec, 3, &nodes, &mut placements);

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
        let nodes = vec![
            make_node(1, "192.168.1.1:8000"),
            make_node(2, "192.168.1.2:8000"),
        ];
        let mut placements = PlacementState::new();
        let spec = make_service_spec(NodeMode::Dedicated, None);

        let decisions = place_service_replicas("api", &spec, 3, &nodes, &mut placements);

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
        let nodes = vec![
            make_node(1, "192.168.1.1:8000"),
            make_node(2, "192.168.1.2:8000"),
        ];
        let mut placements = PlacementState::new();
        let spec = make_service_spec(NodeMode::Exclusive, None);

        // Place 2 replicas - should use both nodes
        let decisions = place_service_replicas("db", &spec, 2, &nodes, &mut placements);

        assert_eq!(decisions.len(), 2);
        assert!(decisions.iter().all(|d| d.is_success()));

        // Now try to place another service - should fail (nodes are exclusive)
        let spec2 = make_service_spec(NodeMode::Exclusive, None);
        let decisions2 = place_service_replicas("cache", &spec2, 1, &nodes, &mut placements);

        assert!(!decisions2[0].is_success());
    }

    #[test]
    fn test_place_with_node_selector() {
        let nodes = vec![
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

        let decisions = place_service_replicas("ml", &spec, 2, &nodes, &mut placements);

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
            &placements
        ));
    }
}
