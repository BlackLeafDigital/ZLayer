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
pub mod metrics;
pub mod raft;
pub mod raft_storage;

pub use autoscaler::{
    Autoscaler, EmaCalculator, ScalingDecision, DEFAULT_COOLDOWN, DEFAULT_EMA_ALPHA,
};
pub use error::{Result, SchedulerError};
pub use metrics::{
    AggregatedMetrics, ContainerdMetricsSource, MetricsCollector, MetricsSource, MockMetricsSource,
    ServiceMetrics,
};
pub use raft::{
    ClusterState, HealthStatus, NodeId, NodeInfo, RaftConfig, RaftCoordinator, Request, Response,
    ScaleEvent, ServiceState, TypeConfig, ZLayerRaft,
};
pub use raft_storage::{LogStore, MemStore, StateMachine};

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

use spec::ScaleSpec;

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
}

impl Scheduler {
    /// Create a new scheduler (standalone mode, no Raft)
    pub fn new_standalone(config: SchedulerConfig) -> Self {
        Self {
            metrics: Arc::new(RwLock::new(MetricsCollector::new())),
            autoscaler: Arc::new(RwLock::new(Autoscaler::new())),
            raft: None,
            config,
            shutdown: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// Create a new scheduler with Raft coordination
    pub async fn new_distributed(config: SchedulerConfig) -> Result<Self> {
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

    /// Apply a scaling decision
    ///
    /// This records the decision and updates state. The actual scaling
    /// (starting/stopping containers) is done by the caller.
    pub async fn apply_scaling(
        &self,
        service_name: &str,
        decision: &ScalingDecision,
    ) -> Result<()> {
        if let Some(target) = decision.target_replicas() {
            // Update autoscaler state
            {
                let mut autoscaler = self.autoscaler.write().await;
                autoscaler.record_scale_action(service_name, target)?;
            }

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
    use spec::ScaleTargets;

    #[tokio::test]
    async fn test_scheduler_standalone() {
        let config = SchedulerConfig::default();
        let scheduler = Scheduler::new_standalone(config);

        // Should always be leader in standalone
        assert!(scheduler.is_leader().await);

        // No cluster state in standalone
        assert!(scheduler.cluster_state().await.is_none());
    }

    #[tokio::test]
    async fn test_scheduler_register_service() {
        let config = SchedulerConfig::default();
        let scheduler = Scheduler::new_standalone(config);

        scheduler
            .register_service("api", ScaleSpec::Fixed { replicas: 3 }, 1)
            .await
            .unwrap();

        assert_eq!(scheduler.current_replicas("api").await, Some(1));
    }

    #[tokio::test]
    async fn test_scheduler_with_mock_metrics() {
        let config = SchedulerConfig::default();
        let scheduler = Scheduler::new_standalone(config);

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
