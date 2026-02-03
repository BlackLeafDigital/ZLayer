//! ZLayer Scheduler - Distributed autoscaling with OpenRaft
//!
//! This crate provides:
//! - **Metrics collection** from container runtimes
//! - **Autoscaling** with EMA-based decision making
//! - **Distributed coordination** via Raft consensus
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
pub mod raft_storage;

#[cfg(feature = "persistent")]
pub mod persistent_raft_storage;

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
    ClusterState, HealthStatus, NodeId, NodeInfo, RaftConfig, RaftCoordinator, Request, Response,
    ScaleEvent, ServiceState, TypeConfig, ZLayerRaft,
};
pub use raft_network::RaftHttpClient;
pub use raft_service::RaftService;
pub use raft_storage::{LogStore, MemStore, StateMachine};

#[cfg(feature = "persistent")]
pub use persistent_raft_storage::{
    PersistentLogStore, PersistentRaftStorage, PersistentStateMachine,
};

use reqwest::Client;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

use zlayer_spec::ScaleSpec;

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
}

impl Scheduler {
    /// Create a new scheduler (standalone mode, no Raft)
    ///
    /// # Arguments
    /// * `config` - Scheduler configuration
    /// * `internal_token` - Token for authenticating with agent internal endpoints
    /// * `agent_base_url` - Base URL of the agent HTTP API (e.g., "http://localhost:8080")
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
        }
    }

    /// Create a new scheduler with Raft coordination
    ///
    /// # Arguments
    /// * `config` - Scheduler configuration
    /// * `internal_token` - Token for authenticating with agent internal endpoints
    /// * `agent_base_url` - Base URL of the agent HTTP API (e.g., "http://localhost:8080")
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
        })
    }

    /// Add a metrics source
    pub async fn add_metrics_source(&self, source: Arc<dyn MetricsSource>) {
        let mut metrics = self.metrics.write().await;
        metrics.add_source(source);
    }

    /// Register a service for scheduling
    pub async fn register_service(
        &self,
        name: impl Into<String>,
        spec: ScaleSpec,
        initial_replicas: u32,
    ) -> Result<()> {
        let name = name.into();

        // Register with autoscaler
        {
            let mut autoscaler = self.autoscaler.write().await;
            autoscaler.register_service(&name, spec.clone(), initial_replicas);
        }

        // If using Raft, also update distributed state
        if let Some(raft) = &self.raft {
            let (min, max) = match &spec {
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

    /// Evaluate scaling for a specific service
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
                    "agent returned status {}: {}",
                    status, body
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

    /// Apply a scaling decision
    ///
    /// This records the decision, updates state, and executes the scaling
    /// by calling the agent's internal endpoint.
    ///
    /// In distributed mode, this verifies we're still the Raft leader before
    /// executing to prevent split-brain scenarios where leadership changed
    /// between evaluation and execution.
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

            // Execute the scaling by calling the agent
            self.execute_scaling_on_agent(service_name, target).await?;

            // If using Raft, record the event
            if let Some(raft) = &self.raft {
                let (from, reason) = match decision {
                    ScalingDecision::ScaleUp { from, reason, .. } => (*from, reason.clone()),
                    ScalingDecision::ScaleDown { from, reason, .. } => (*from, reason.clone()),
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

    /// Run the scheduling loop
    ///
    /// This continuously collects metrics and makes scaling decisions.
    /// Returns when shutdown is signaled.
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
                _ = self.shutdown.notified() => {
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
    pub async fn is_leader(&self) -> bool {
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
            "http://localhost:8080".to_string(),
        );

        // Should always be leader in standalone
        assert!(scheduler.is_leader().await);

        // No cluster state in standalone
        assert!(scheduler.cluster_state().await.is_none());
    }

    #[tokio::test]
    async fn test_scheduler_register_service() {
        let config = SchedulerConfig::default();
        let scheduler = Scheduler::new_standalone(
            config,
            "test-token".to_string(),
            "http://localhost:8080".to_string(),
        );

        scheduler
            .register_service("api", ScaleSpec::Fixed { replicas: 3 }, 1)
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
            "http://localhost:8080".to_string(),
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
            )
            .await
            .unwrap();

        // Evaluate - should want to scale up due to high CPU
        let decision = scheduler.evaluate_service("api").await.unwrap();

        match decision {
            ScalingDecision::ScaleUp { from: 2, to: 3, .. } => {}
            other => panic!("Expected ScaleUp, got {:?}", other),
        }
    }
}
