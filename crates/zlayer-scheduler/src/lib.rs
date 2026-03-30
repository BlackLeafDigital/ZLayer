//! `ZLayer` Scheduler - Distributed autoscaling with `OpenRaft`
//!
//! This crate provides:
//! - **Metrics collection** from container runtimes
//! - **Autoscaling** with EMA-based decision making
//! - **Distributed coordination** via Raft consensus (using `zlayer-consensus`)
//!
//! # Architecture
//!
//! The scheduler runs on each node in the cluster. One node is elected leader
//! via Raft and makes scaling decisions. All nodes collect metrics and report
//! to the leader.
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                      Scheduler                              │
//! │  ┌─────────────┐  ┌────────────┐  ┌──────────────────┐    │
//! │  │  Metrics    │  │ Autoscaler │  │ RaftCoordinator  │    │
//! │  │  Collector  │──│            │──│   (consensus)    │    │
//! │  └─────────────┘  └────────────┘  └──────────────────┘    │
//! └─────────────────────────────────────────────────────────────┘
//! ```

pub mod autoscaler;
pub mod error;
pub mod handlers;
pub mod metrics;
pub mod placement;
pub mod raft;
pub mod raft_network;
pub mod raft_service;
#[cfg(feature = "persistent")]
pub mod raft_storage;

pub use autoscaler::{
    Autoscaler, EmaCalculator, ScalingDecision, DEFAULT_COOLDOWN, DEFAULT_EMA_ALPHA,
};
pub use error::{Result, SchedulerError};
pub use metrics::{
    AggregatedMetrics, ContainerdMetricsSource, MetricsCollector, MetricsSource, MockMetricsSource,
    ServiceMetrics,
};
pub use placement::{
    can_place_on_node, place_service_replicas, validate_placement_feasibility, ContainerId,
    NodeResources, NodeState, PlacementDecision, PlacementReason, PlacementState,
};
pub use raft::{
    force_leader_marker_path, load_and_clear_force_leader_state, save_force_leader_state,
    target_voters, AddMemberParams, ClusterState, GpuInfoSummary, HealthStatus, MemberRole, NodeId,
    NodeInfo, RaftConfig, RaftCoordinator, Request, Response, ScaleEvent, ServiceState, TypeConfig,
    ZLayerRaft,
};
pub use raft_network::RaftHttpClient;
pub use raft_service::RaftService;

use reqwest::Client;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

use zlayer_spec::{ScaleSpec, ServiceSpec};

/// Configuration for the scheduler
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// Raft configuration
    pub raft: RaftConfig,
    /// Metrics collection interval
    pub metrics_interval: Duration,
    /// Whether this node should try to bootstrap a new cluster
    pub bootstrap: bool,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            raft: RaftConfig::default(),
            metrics_interval: Duration::from_secs(10),
            bootstrap: false,
        }
    }
}

/// Main scheduler orchestrator
///
/// Coordinates metrics collection, autoscaling decisions, and distributed
/// state management across the cluster.
pub struct Scheduler {
    /// Metrics collector
    metrics: Arc<RwLock<MetricsCollector>>,
    /// Autoscaler
    autoscaler: Arc<RwLock<Autoscaler>>,
    /// Raft coordinator (optional, only if distributed)
    raft: Option<Arc<RaftCoordinator>>,
    /// Configuration
    config: SchedulerConfig,
    /// Shutdown signal
    shutdown: Arc<tokio::sync::Notify>,
    /// HTTP client for agent communication
    #[cfg_attr(feature = "test-skip-http", allow(dead_code))]
    http_client: Client,
    /// Internal token for authenticating with agents
    #[cfg_attr(feature = "test-skip-http", allow(dead_code))]
    internal_token: String,
    /// Base URL of the agent's HTTP API
    #[cfg_attr(feature = "test-skip-http", allow(dead_code))]
    agent_base_url: String,
    /// Placement state tracking where containers are placed across nodes
    placement_state: Arc<RwLock<PlacementState>>,
    /// Service specs for placement decisions (`service_name` -> `ServiceSpec`)
    service_specs: Arc<RwLock<HashMap<String, ServiceSpec>>>,
}

impl Scheduler {
    /// Create a new scheduler (standalone mode, no Raft)
    ///
    /// # Arguments
    /// * `config` - Scheduler configuration
    /// * `internal_token` - Token for authenticating with agent internal endpoints
    /// * `agent_base_url` - Base URL of the agent HTTP API (e.g., "<http://localhost:3669>")
    #[must_use]
    pub fn new_standalone(
        config: SchedulerConfig,
        internal_token: String,
        agent_base_url: String,
    ) -> Self {
        Self {
            metrics: Arc::new(RwLock::new(MetricsCollector::new())),
            autoscaler: Arc::new(RwLock::new(Autoscaler::new())),
            raft: None,
            config,
            shutdown: Arc::new(tokio::sync::Notify::new()),
            http_client: Client::new(),
            internal_token,
            agent_base_url,
            placement_state: Arc::new(RwLock::new(PlacementState::new())),
            service_specs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a new scheduler with Raft coordination.
    ///
    /// # Arguments
    /// * `config` - Scheduler configuration
    /// * `internal_token` - Token for authenticating with agent internal endpoints
    /// * `agent_base_url` - Base URL of the agent HTTP API (e.g., "<http://localhost:3669>")
    ///
    /// # Errors
    ///
    /// Returns an error if the Raft coordinator fails to initialize or bootstrap.
    pub async fn new_distributed(
        config: SchedulerConfig,
        internal_token: String,
        agent_base_url: String,
    ) -> Result<Self> {
        let raft = RaftCoordinator::new(config.raft.clone()).await?;

        if config.bootstrap {
            raft.bootstrap().await?;
        }

        Ok(Self {
            metrics: Arc::new(RwLock::new(MetricsCollector::new())),
            autoscaler: Arc::new(RwLock::new(Autoscaler::new())),
            raft: Some(Arc::new(raft)),
            config,
            shutdown: Arc::new(tokio::sync::Notify::new()),
            http_client: Client::new(),
            internal_token,
            agent_base_url,
            placement_state: Arc::new(RwLock::new(PlacementState::new())),
            service_specs: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Create a new scheduler with an existing Raft coordinator.
    ///
    /// Used when the daemon has already initialised a `RaftCoordinator` and
    /// wants to share it with the scheduler (e.g. for node-death rescheduling).
    ///
    /// # Arguments
    /// * `config` - Scheduler configuration
    /// * `raft` - Pre-existing Raft coordinator
    /// * `internal_token` - Token for authenticating with agent internal endpoints
    /// * `agent_base_url` - Base URL of the agent HTTP API (e.g., "<http://localhost:3669>")
    #[must_use]
    pub fn with_raft(
        config: SchedulerConfig,
        raft: Arc<RaftCoordinator>,
        internal_token: String,
        agent_base_url: String,
    ) -> Self {
        Self {
            metrics: Arc::new(RwLock::new(MetricsCollector::new())),
            autoscaler: Arc::new(RwLock::new(Autoscaler::new())),
            raft: Some(raft),
            config,
            shutdown: Arc::new(tokio::sync::Notify::new()),
            http_client: Client::new(),
            internal_token,
            agent_base_url,
            placement_state: Arc::new(RwLock::new(PlacementState::new())),
            service_specs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Add a metrics source
    pub async fn add_metrics_source(&self, source: Arc<dyn MetricsSource>) {
        let mut metrics = self.metrics.write().await;
        metrics.add_source(source);
    }

    /// Register a service for scheduling.
    ///
    /// # Arguments
    /// * `name` - Service name
    /// * `scale_spec` - Scaling specification (fixed, adaptive, manual)
    /// * `initial_replicas` - Starting replica count
    /// * `service_spec` - Optional full service spec for placement decisions
    ///
    /// # Errors
    ///
    /// Returns an error if the Raft proposal to register the service fails.
    pub async fn register_service(
        &self,
        name: impl Into<String>,
        scale_spec: ScaleSpec,
        initial_replicas: u32,
        service_spec: Option<ServiceSpec>,
    ) -> Result<()> {
        let name = name.into();

        // Register with autoscaler
        {
            let mut autoscaler = self.autoscaler.write().await;
            autoscaler.register_service(&name, scale_spec.clone(), initial_replicas);
        }

        // Store the service spec for placement decisions
        if let Some(spec) = service_spec {
            let mut specs = self.service_specs.write().await;
            specs.insert(name.clone(), spec);
        }

        // If using Raft, also update distributed state
        if let Some(raft) = &self.raft {
            let (min, max) = match &scale_spec {
                ScaleSpec::Adaptive { min, max, .. } => (*min, *max),
                ScaleSpec::Fixed { replicas } => (*replicas, *replicas),
                ScaleSpec::Manual => (0, u32::MAX),
            };

            raft.update_service(
                name.clone(),
                ServiceState {
                    current_replicas: initial_replicas,
                    desired_replicas: initial_replicas,
                    min_replicas: min,
                    max_replicas: max,
                    health_status: HealthStatus::Unknown,
                    last_scale_time: None,
                    assigned_nodes: vec![self.config.raft.node_id],
                },
            )
            .await?;
        }

        info!(service = %name, initial_replicas, "Registered service");
        Ok(())
    }

    /// Unregister a service
    pub async fn unregister_service(&self, name: &str) {
        let mut autoscaler = self.autoscaler.write().await;
        autoscaler.unregister_service(name);
        info!(service = name, "Unregistered service");
    }

    /// Evaluate scaling for a specific service.
    ///
    /// # Errors
    ///
    /// Returns an error if metrics collection or autoscaler evaluation fails.
    pub async fn evaluate_service(&self, service_name: &str) -> Result<ScalingDecision> {
        // Collect metrics
        let aggregated = {
            let metrics = self.metrics.read().await;
            metrics.collect(service_name).await?
        };

        // Make scaling decision
        let decision = {
            let mut autoscaler = self.autoscaler.write().await;
            autoscaler.evaluate(service_name, &aggregated)?
        };

        debug!(
            service = service_name,
            ?decision,
            cpu = aggregated.avg_cpu_percent,
            memory = aggregated.avg_memory_percent,
            "Evaluated scaling"
        );

        Ok(decision)
    }

    /// Execute a scaling decision by calling the agent's internal endpoint
    ///
    /// This sends a scaling request to the agent to actually start/stop containers.
    async fn execute_scaling_on_agent(&self, service: &str, replicas: u32) -> Result<()> {
        // Skip HTTP calls during tests
        #[cfg(feature = "test-skip-http")]
        {
            debug!(
                service = service,
                replicas = replicas,
                "Test mode: skipping agent scaling call"
            );
            Ok(())
        }

        #[cfg(not(feature = "test-skip-http"))]
        {
            let url = format!("{}/api/v1/internal/scale", self.agent_base_url);

            debug!(
                service = service,
                replicas = replicas,
                url = %url,
                "Sending scaling request to agent"
            );

            let response = self
                .http_client
                .post(&url)
                .header("X-ZLayer-Internal-Token", &self.internal_token)
                .json(&serde_json::json!({
                    "service": service,
                    "replicas": replicas
                }))
                .send()
                .await
                .map_err(|e| SchedulerError::AgentCommunication(e.to_string()))?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response
                    .text()
                    .await
                    .unwrap_or_else(|_| "unknown".to_string());
                return Err(SchedulerError::AgentCommunication(format!(
                    "agent returned status {status}: {body}"
                )));
            }

            info!(
                service = service,
                replicas = replicas,
                "Successfully sent scaling request to agent"
            );

            Ok(())
        }
    }

    /// Build placement-ready node states from current Raft cluster state.
    ///
    /// Filters to only "ready" nodes and maps their resource information
    /// into `NodeState` structs suitable for the placement algorithm.
    async fn build_node_states(&self) -> Vec<placement::NodeState> {
        let cluster = match &self.raft {
            Some(raft) => raft.read_state().await,
            None => return vec![],
        };

        cluster
            .nodes
            .values()
            .filter(|n| n.status == "ready")
            .map(|n| {
                let mut resources = placement::NodeResources::new(n.cpu_total, n.memory_total);
                resources.cpu_used = n.cpu_used;
                resources.memory_used = n.memory_used;
                // Map GPU info from the Raft node info
                #[allow(clippy::cast_possible_truncation)]
                let gpu_count = n.gpus.len() as u32;
                resources.gpu_total = gpu_count;
                resources.gpu_models = n.gpus.iter().map(|g| g.model.clone()).collect();
                resources.gpu_memory_mb = n.gpus.iter().map(|g| g.memory_mb).sum();
                if let Some(first_gpu) = n.gpus.first() {
                    resources.gpu_vendor.clone_from(&first_gpu.vendor);
                }
                resources.gpu_allocated = vec![placement::GpuAllocation::Free; gpu_count as usize];

                placement::NodeState {
                    id: n.node_id,
                    address: n.advertise_addr.clone(),
                    labels: std::collections::HashMap::new(),
                    resources,
                    healthy: true,
                }
            })
            .collect()
    }

    /// Compute where to place service replicas across available nodes.
    ///
    /// Uses the placement module's `place_service_replicas` to run bin-packing,
    /// dedicated, or exclusive placement depending on the service's `node_mode`.
    ///
    /// Returns a map of `node_id` -> list of container IDs assigned to that node.
    #[allow(clippy::unused_self)]
    fn compute_placement(
        &self,
        service_name: &str,
        desired_replicas: u32,
        nodes: &mut [placement::NodeState],
        placement_state: &mut placement::PlacementState,
        spec: Option<&ServiceSpec>,
    ) -> HashMap<NodeId, Vec<placement::ContainerId>> {
        // Build a default ServiceSpec if none provided, using Shared mode
        let default_spec = ServiceSpec {
            rtype: zlayer_spec::ResourceType::Service,
            schedule: None,
            image: zlayer_spec::ImageSpec {
                name: "unknown:latest".to_string(),
                pull_policy: zlayer_spec::PullPolicy::IfNotPresent,
            },
            resources: zlayer_spec::ResourcesSpec::default(),
            env: HashMap::default(),
            command: zlayer_spec::CommandSpec::default(),
            network: zlayer_spec::NetworkSpec::default(),
            endpoints: vec![],
            scale: zlayer_spec::ScaleSpec::default(),
            depends: vec![],
            health: zlayer_spec::HealthSpec {
                start_grace: None,
                interval: None,
                timeout: None,
                retries: 3,
                check: zlayer_spec::HealthCheck::Tcp { port: 0 },
            },
            init: zlayer_spec::InitSpec::default(),
            errors: zlayer_spec::ErrorsSpec::default(),
            devices: vec![],
            storage: vec![],
            capabilities: vec![],
            privileged: false,
            node_mode: zlayer_spec::NodeMode::Shared,
            node_selector: None,
            service_type: zlayer_spec::ServiceType::default(),
            wasm: None,
            host_network: false,
        };

        let effective_spec = spec.unwrap_or(&default_spec);

        let decisions = placement::place_service_replicas(
            service_name,
            effective_spec,
            desired_replicas,
            nodes,
            placement_state,
        );

        // Check for failed placements
        let failed: Vec<_> = decisions.iter().filter(|d| !d.is_success()).collect();

        if !failed.is_empty() {
            let failed_count = failed.len();
            let total = decisions.len();
            warn!(
                service = service_name,
                failed = failed_count,
                total,
                "Some replicas could not be placed"
            );
        }

        // Group successful placements by node
        let mut node_assignments: HashMap<NodeId, Vec<placement::ContainerId>> = HashMap::new();
        for decision in &decisions {
            if let Some(node_id) = decision.node_id {
                node_assignments
                    .entry(node_id)
                    .or_default()
                    .push(decision.container_id.clone());
            }
        }

        node_assignments
    }

    /// Execute scaling across multiple nodes by calling each node's internal API.
    ///
    /// The leader dispatches scale requests to each node that has been assigned
    /// containers for this service. Uses the same internal API endpoint and
    /// authentication as local scaling.
    #[allow(clippy::too_many_lines)]
    async fn execute_distributed_scaling(
        &self,
        service_name: &str,
        node_assignments: &HashMap<NodeId, Vec<placement::ContainerId>>,
    ) -> Result<()> {
        let cluster = match &self.raft {
            Some(raft) => raft.read_state().await,
            None => return Err(SchedulerError::NotLeader),
        };

        for (node_id, containers) in node_assignments {
            let Some(node_info) = cluster.nodes.get(node_id) else {
                warn!(
                    node_id,
                    "Node not found in cluster state, skipping dispatch"
                );
                continue;
            };

            #[allow(clippy::cast_possible_truncation)]
            let replicas = containers.len() as u32;

            // Determine the target URL for this node's internal API
            let addr = if node_info.advertise_addr.is_empty() {
                // Fall back to raft address (strip port, use api_port)
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
                3669 // Default ZLayer API port
            };

            let url = format!("http://{addr}:{port}/api/v1/internal/scale");

            info!(
                service = service_name,
                node_id,
                replicas,
                url = %url,
                "Dispatching scale to remote node"
            );

            // Skip HTTP calls during tests
            #[cfg(not(feature = "test-skip-http"))]
            {
                match self
                    .http_client
                    .post(&url)
                    .header("X-ZLayer-Internal-Token", &self.internal_token)
                    .json(&serde_json::json!({
                        "service": service_name,
                        "replicas": replicas,
                    }))
                    .timeout(Duration::from_secs(30))
                    .send()
                    .await
                {
                    Ok(resp) if resp.status().is_success() => {
                        info!(
                            node_id,
                            service = service_name,
                            replicas,
                            "Scale dispatch succeeded"
                        );
                    }
                    Ok(resp) => {
                        let status = resp.status();
                        let body = resp.text().await.unwrap_or_default();
                        error!(
                            node_id,
                            service = service_name,
                            status = %status,
                            body,
                            "Scale dispatch failed"
                        );
                    }
                    Err(e) => {
                        error!(
                            node_id,
                            service = service_name,
                            error = %e,
                            "Scale dispatch request failed"
                        );
                    }
                }
            }

            #[cfg(feature = "test-skip-http")]
            {
                debug!(
                    node_id,
                    service = service_name,
                    replicas,
                    "Test mode: skipping remote scale dispatch"
                );
            }
        }

        // Update service assignments in Raft so cluster state tracks which
        // nodes are running this service
        if let Some(ref raft) = self.raft {
            let assigned_nodes: Vec<NodeId> = node_assignments.keys().copied().collect();
            let _ = raft
                .propose(Request::UpdateServiceAssignment {
                    service_name: service_name.to_string(),
                    node_ids: assigned_nodes,
                })
                .await;
        }

        Ok(())
    }

    /// Apply a scaling decision.
    ///
    /// This records the decision, updates state, and executes the scaling.
    ///
    /// In distributed mode (Raft enabled with multiple ready nodes), this uses
    /// the placement algorithm to distribute replicas across nodes and dispatches
    /// scale requests to each node's internal API.
    ///
    /// In standalone mode or single-node clusters, this falls back to calling
    /// the local agent directly.
    ///
    /// # Errors
    ///
    /// Returns an error if recording the scale action, executing scaling on
    /// the agent, or recording the Raft scale event fails.
    pub async fn apply_scaling(
        &self,
        service_name: &str,
        decision: &ScalingDecision,
    ) -> Result<()> {
        // Double-check leader status before executing scaling
        // This prevents race conditions where leadership changed after evaluation
        if let Some(raft) = &self.raft {
            if !raft.is_leader() {
                warn!(
                    service = %service_name,
                    "Skipping scaling execution - no longer leader"
                );
                return Ok(());
            }
        }

        if let Some(target) = decision.target_replicas() {
            // Update autoscaler state
            {
                let mut autoscaler = self.autoscaler.write().await;
                autoscaler.record_scale_action(service_name, target)?;
            }

            // Determine whether to use distributed placement or local-only scaling
            if self.raft.is_some() {
                // Distributed mode: use placement algorithm across cluster nodes
                let mut nodes = self.build_node_states().await;

                if nodes.len() > 1 {
                    // Multi-node cluster: compute placement and dispatch to nodes
                    let spec = {
                        let specs = self.service_specs.read().await;
                        specs.get(service_name).cloned()
                    };

                    let node_assignments = {
                        let mut ps = self.placement_state.write().await;
                        self.compute_placement(
                            service_name,
                            target,
                            &mut nodes,
                            &mut ps,
                            spec.as_ref(),
                        )
                    };

                    if node_assignments.is_empty() {
                        warn!(
                            service = service_name,
                            target, "Placement found no suitable nodes, falling back to local"
                        );
                        self.execute_scaling_on_agent(service_name, target).await?;
                    } else {
                        info!(
                            service = service_name,
                            target,
                            nodes_used = node_assignments.len(),
                            "Distributed placement computed"
                        );
                        self.execute_distributed_scaling(service_name, &node_assignments)
                            .await?;
                    }
                } else {
                    // Single-node Raft cluster: fall back to local agent
                    debug!(
                        service = service_name,
                        "Single-node cluster, using local scaling"
                    );
                    self.execute_scaling_on_agent(service_name, target).await?;
                }
            } else {
                // Standalone mode: call local agent directly
                self.execute_scaling_on_agent(service_name, target).await?;
            }

            // If using Raft, record the event
            if let Some(raft) = &self.raft {
                let (from, reason) = match decision {
                    ScalingDecision::ScaleUp { from, reason, .. }
                    | ScalingDecision::ScaleDown { from, reason, .. } => (*from, reason.clone()),
                    _ => return Ok(()),
                };

                raft.record_scale_event(service_name.to_string(), from, target, reason)
                    .await?;
            }

            info!(
                service = service_name,
                target_replicas = target,
                "Applied scaling decision"
            );
        }

        Ok(())
    }

    /// Run the scheduling loop.
    ///
    /// This continuously collects metrics and makes scaling decisions.
    /// Returns when shutdown is signaled.
    ///
    /// # Errors
    ///
    /// Returns an error if Raft shutdown fails.
    pub async fn run(&self, services: Vec<String>) -> Result<()> {
        let mut ticker = interval(self.config.metrics_interval);

        info!(
            interval_secs = self.config.metrics_interval.as_secs(),
            service_count = services.len(),
            "Starting scheduler loop"
        );

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    // Only the leader makes scaling decisions in distributed mode
                    if let Some(raft) = &self.raft {
                        if !raft.is_leader() {
                            debug!("Not leader, skipping scaling evaluation");
                            continue;
                        }
                    }

                    for service in &services {
                        match self.evaluate_service(service).await {
                            Ok(decision) => {
                                if decision.is_change() {
                                    if let Err(e) = self.apply_scaling(service, &decision).await {
                                        error!(service = %service, error = %e, "Failed to apply scaling");
                                    }
                                }
                            }
                            Err(e) => {
                                warn!(service = %service, error = %e, "Failed to evaluate scaling");
                            }
                        }
                    }
                }
                () = self.shutdown.notified() => {
                    info!("Scheduler shutdown requested");
                    break;
                }
            }
        }

        // Cleanup
        if let Some(raft) = &self.raft {
            raft.shutdown().await?;
        }

        Ok(())
    }

    /// Signal shutdown
    pub fn shutdown(&self) {
        self.shutdown.notify_one();
    }

    /// Check if this node is the leader (always true for standalone)
    #[must_use]
    pub fn is_leader(&self) -> bool {
        match &self.raft {
            Some(raft) => raft.is_leader(),
            None => true, // Standalone is always "leader"
        }
    }

    /// Get the Prometheus registry for metrics exposition
    pub async fn metrics_registry(&self) -> prometheus::Registry {
        let metrics = self.metrics.read().await;
        metrics.registry().clone()
    }

    /// Get current replica count for a service
    pub async fn current_replicas(&self, service_name: &str) -> Option<u32> {
        let autoscaler = self.autoscaler.read().await;
        autoscaler.current_replicas(service_name)
    }

    /// Get cluster state (if distributed)
    pub async fn cluster_state(&self) -> Option<ClusterState> {
        match &self.raft {
            Some(raft) => Some(raft.read_state().await),
            None => None,
        }
    }

    /// Handle a node death by rescheduling its containers to remaining live nodes.
    ///
    /// Called by the dead-node detection loop after marking a node as "dead".
    /// Finds all services that had replicas on the dead node, clears their
    /// stale placements, recomputes placement across remaining live nodes,
    /// and dispatches scaling requests.
    ///
    /// # Errors
    ///
    /// Returns `SchedulerError::NotLeader` if Raft is not configured, or
    /// an error if distributed scaling dispatch fails.
    pub async fn handle_node_death(&self, dead_node_id: NodeId) -> Result<()> {
        let raft = self.raft.as_ref().ok_or(SchedulerError::NotLeader)?;
        let state = raft.read_state().await;

        // Find all services that had replicas on the dead node
        let affected_services: Vec<(String, u32)> = state
            .services
            .iter()
            .filter(|(_, svc)| svc.assigned_nodes.contains(&dead_node_id))
            .map(|(name, svc)| (name.clone(), svc.desired_replicas))
            .collect();

        if affected_services.is_empty() {
            info!(node_id = dead_node_id, "Dead node had no assigned services");
            return Ok(());
        }

        warn!(
            node_id = dead_node_id,
            services = ?affected_services.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            "Rescheduling services from dead node"
        );

        // Get live nodes for placement
        let nodes = self.build_node_states().await;
        if nodes.is_empty() {
            error!("No live nodes available for rescheduling!");
            return Ok(());
        }

        // Reschedule each affected service
        for (service_name, desired_replicas) in affected_services {
            let spec = {
                let specs = self.service_specs.read().await;
                specs.get(&service_name).cloned()
            };

            let node_assignments = {
                let mut placement_state = self.placement_state.write().await;

                // Clear stale placements for this service from the dead node
                let removed = placement_state.remove_service_from_node(dead_node_id, &service_name);
                if !removed.is_empty() {
                    debug!(
                        service = %service_name,
                        dead_node = dead_node_id,
                        removed_count = removed.len(),
                        "Cleared stale placements from dead node"
                    );
                }

                self.compute_placement(
                    &service_name,
                    desired_replicas,
                    &mut nodes.clone(),
                    &mut placement_state,
                    spec.as_ref(),
                )
            };

            if node_assignments.is_empty() {
                warn!(
                    service = %service_name,
                    "No suitable nodes found for rescheduling"
                );
                continue;
            }

            info!(
                service = %service_name,
                nodes_used = node_assignments.len(),
                desired_replicas,
                "Rescheduling placement computed"
            );

            if let Err(e) = self
                .execute_distributed_scaling(&service_name, &node_assignments)
                .await
            {
                error!(
                    service = %service_name,
                    error = %e,
                    "Failed to execute rescheduling"
                );
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zlayer_spec::ScaleTargets;

    #[tokio::test]
    async fn test_scheduler_standalone() {
        let config = SchedulerConfig::default();
        let scheduler = Scheduler::new_standalone(
            config,
            "test-token".to_string(),
            "http://localhost:3669".to_string(),
        );

        // Should always be leader in standalone
        assert!(scheduler.is_leader());

        // No cluster state in standalone
        assert!(scheduler.cluster_state().await.is_none());
    }

    #[tokio::test]
    async fn test_scheduler_register_service() {
        let config = SchedulerConfig::default();
        let scheduler = Scheduler::new_standalone(
            config,
            "test-token".to_string(),
            "http://localhost:3669".to_string(),
        );

        scheduler
            .register_service("api", ScaleSpec::Fixed { replicas: 3 }, 1, None)
            .await
            .unwrap();

        assert_eq!(scheduler.current_replicas("api").await, Some(1));
    }

    #[tokio::test]
    async fn test_scheduler_with_mock_metrics() {
        let config = SchedulerConfig::default();
        let scheduler = Scheduler::new_standalone(
            config,
            "test-token".to_string(),
            "http://localhost:3669".to_string(),
        );

        // Add mock metrics source
        let mock = Arc::new(MockMetricsSource::new());
        mock.set_metrics(
            "api",
            vec![ServiceMetrics {
                cpu_percent: 80.0,
                memory_bytes: 512 * 1024 * 1024,
                memory_limit: 1024 * 1024 * 1024,
                rps: Some(100.0),
                timestamp: Some(std::time::Instant::now()),
            }],
        )
        .await;

        scheduler.add_metrics_source(mock).await;

        // Register service with adaptive scaling
        scheduler
            .register_service(
                "api",
                ScaleSpec::Adaptive {
                    min: 1,
                    max: 10,
                    cooldown: Some(Duration::from_secs(0)),
                    targets: ScaleTargets {
                        cpu: Some(70),
                        memory: None,
                        rps: None,
                    },
                },
                2,
                None,
            )
            .await
            .unwrap();

        // Evaluate - should want to scale up due to high CPU
        let decision = scheduler.evaluate_service("api").await.unwrap();

        match decision {
            ScalingDecision::ScaleUp { from: 2, to: 3, .. } => {}
            other => panic!("Expected ScaleUp, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_handle_node_death_standalone_returns_error() {
        let config = SchedulerConfig::default();
        let scheduler = Scheduler::new_standalone(
            config,
            "test-token".to_string(),
            "http://localhost:3669".to_string(),
        );

        // handle_node_death requires Raft, so standalone should fail with NotLeader
        let result = scheduler.handle_node_death(42).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, SchedulerError::NotLeader),
            "Expected NotLeader, got: {err:?}"
        );
    }
}
