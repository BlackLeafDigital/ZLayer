//! AutoscaleController - Connects autoscaling decisions to container scaling
//!
//! This module provides an `AutoscaleController` that bridges the scheduler's
//! autoscaling logic with the agent's `ServiceManager` to automatically scale
//! services based on resource utilization.
//!
//! # Architecture
//!
//! ```text
//! ┌────────────────────────────────────────────────────────────────────┐
//! │                     AutoscaleController                            │
//! │  ┌─────────────────┐  ┌────────────┐  ┌──────────────────┐       │
//! │  │ CgroupsMetrics  │  │ Autoscaler │  │ ServiceManager   │       │
//! │  │    Source       │──│            │──│  (scaling)       │       │
//! │  └─────────────────┘  └────────────┘  └──────────────────┘       │
//! └────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Example
//!
//! ```ignore
//! use zlayer_agent::autoscale_controller::AutoscaleController;
//! use zlayer_agent::{ServiceManager, RuntimeConfig, create_runtime};
//! use std::sync::Arc;
//! use std::time::Duration;
//!
//! // Create runtime and service manager
//! let runtime = create_runtime(RuntimeConfig::Mock).await?;
//! let manager = Arc::new(ServiceManager::new(runtime.clone()));
//!
//! // Create autoscale controller
//! let controller = AutoscaleController::new(
//!     manager.clone(),
//!     runtime.clone(),
//!     Duration::from_secs(10),
//! );
//!
//! // Register services with adaptive scaling
//! controller.register_service("api", &scale_spec, 2).await;
//!
//! // Run the autoscaling loop (in background)
//! let handle = tokio::spawn(async move {
//!     controller.run_loop().await
//! });
//!
//! // Later, shutdown
//! controller.shutdown();
//! ```

use crate::error::Result;
use crate::metrics_providers::{RuntimeStatsProvider, ServiceManagerContainerProvider};
use crate::runtime::Runtime;
use crate::service::ServiceManager;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use zlayer_scheduler::metrics::{CgroupsMetricsSource, MetricsCollector, MetricsSource};
use zlayer_scheduler::Autoscaler;
use zlayer_spec::ScaleSpec;

/// Default autoscaling evaluation interval
pub const DEFAULT_AUTOSCALE_INTERVAL: Duration = Duration::from_secs(10);

/// Controller that connects autoscaling decisions to actual container scaling
///
/// The `AutoscaleController` periodically collects metrics from running containers,
/// evaluates whether scaling is needed using the `Autoscaler`, and executes scaling
/// decisions through the `ServiceManager`.
pub struct AutoscaleController {
    /// Service manager for executing scaling operations
    service_manager: Arc<ServiceManager>,
    /// Metrics collector with cgroups source
    metrics: Arc<MetricsCollector>,
    /// Autoscaler decision engine
    autoscaler: Arc<RwLock<Autoscaler>>,
    /// Service specs for scale configuration (service_name -> spec)
    service_specs: Arc<RwLock<HashMap<String, ScaleSpec>>>,
    /// Last scale times for cooldown tracking (service_name -> instant)
    last_scale_times: Arc<RwLock<HashMap<String, Instant>>>,
    /// Evaluation interval
    interval: Duration,
    /// Shutdown signal
    shutdown: Arc<tokio::sync::Notify>,
}

impl AutoscaleController {
    /// Create a new autoscale controller
    ///
    /// # Arguments
    /// * `service_manager` - The service manager used to execute scaling operations
    /// * `runtime` - The container runtime for collecting metrics
    /// * `interval` - How often to evaluate scaling decisions
    ///
    /// # Example
    ///
    /// ```ignore
    /// let controller = AutoscaleController::new(
    ///     service_manager,
    ///     runtime,
    ///     Duration::from_secs(10),
    /// );
    /// ```
    pub fn new(
        service_manager: Arc<ServiceManager>,
        runtime: Arc<dyn Runtime + Send + Sync>,
        interval: Duration,
    ) -> Self {
        // Create metrics collector with cgroups source
        let mut metrics = MetricsCollector::new();

        // Create the stats provider wrapping the runtime
        let stats_provider = Arc::new(RuntimeStatsProvider::new(runtime));

        // Create the service container provider wrapping the service manager
        let service_provider = Arc::new(ServiceManagerContainerProvider::new(
            service_manager.clone(),
        ));

        // Create cgroups metrics source
        let source: Arc<dyn MetricsSource> =
            Arc::new(CgroupsMetricsSource::new(service_provider, stats_provider));
        metrics.add_source(source);

        Self {
            service_manager,
            metrics: Arc::new(metrics),
            autoscaler: Arc::new(RwLock::new(Autoscaler::new())),
            service_specs: Arc::new(RwLock::new(HashMap::new())),
            last_scale_times: Arc::new(RwLock::new(HashMap::new())),
            interval,
            shutdown: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// Create with a custom metrics collector (useful for testing)
    pub fn with_custom_metrics(
        service_manager: Arc<ServiceManager>,
        metrics: MetricsCollector,
        interval: Duration,
    ) -> Self {
        Self {
            service_manager,
            metrics: Arc::new(metrics),
            autoscaler: Arc::new(RwLock::new(Autoscaler::new())),
            service_specs: Arc::new(RwLock::new(HashMap::new())),
            last_scale_times: Arc::new(RwLock::new(HashMap::new())),
            interval,
            shutdown: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// Register a service for autoscaling
    ///
    /// Only services with `ScaleSpec::Adaptive` will be evaluated for autoscaling.
    /// Services with `Fixed` or `Manual` scaling are ignored by the autoscaler loop.
    ///
    /// # Arguments
    /// * `name` - Service name
    /// * `spec` - The service's scale specification
    /// * `initial_replicas` - Current number of replicas
    pub async fn register_service(&self, name: &str, spec: &ScaleSpec, initial_replicas: u32) {
        // Only register adaptive services
        if !matches!(spec, ScaleSpec::Adaptive { .. }) {
            debug!(
                service = name,
                "Skipping registration for non-adaptive service"
            );
            return;
        }

        // Register with autoscaler
        {
            let mut autoscaler = self.autoscaler.write().await;
            autoscaler.register_service(name, spec.clone(), initial_replicas);
        }

        // Store spec for reference
        {
            let mut specs = self.service_specs.write().await;
            specs.insert(name.to_string(), spec.clone());
        }

        info!(
            service = name,
            initial_replicas, "Registered service for autoscaling"
        );
    }

    /// Unregister a service from autoscaling
    pub async fn unregister_service(&self, name: &str) {
        {
            let mut autoscaler = self.autoscaler.write().await;
            autoscaler.unregister_service(name);
        }

        {
            let mut specs = self.service_specs.write().await;
            specs.remove(name);
        }

        {
            let mut times = self.last_scale_times.write().await;
            times.remove(name);
        }

        info!(service = name, "Unregistered service from autoscaling");
    }

    /// Check if a service is registered for autoscaling
    pub async fn is_registered(&self, name: &str) -> bool {
        let specs = self.service_specs.read().await;
        specs.contains_key(name)
    }

    /// Check if a service is in cooldown period
    ///
    /// Returns true if the service was scaled recently and is still in cooldown.
    async fn should_scale(&self, service_name: &str) -> bool {
        // Get the cooldown duration from the spec
        let cooldown = {
            let specs = self.service_specs.read().await;
            match specs.get(service_name) {
                Some(ScaleSpec::Adaptive { cooldown, .. }) => {
                    cooldown.unwrap_or(zlayer_scheduler::DEFAULT_COOLDOWN)
                }
                _ => return false, // Not adaptive, shouldn't scale
            }
        };

        // Check if we're past the cooldown period
        let last_scale_times = self.last_scale_times.read().await;
        if let Some(last_time) = last_scale_times.get(service_name) {
            if last_time.elapsed() < cooldown {
                let remaining = cooldown
                    .checked_sub(last_time.elapsed())
                    .unwrap_or_default();
                debug!(
                    service = service_name,
                    remaining_secs = remaining.as_secs(),
                    "Service in cooldown"
                );
                return false;
            }
        }

        true
    }

    /// Record that a scale action occurred
    async fn record_scale_action(&self, service_name: &str) {
        let mut times = self.last_scale_times.write().await;
        times.insert(service_name.to_string(), Instant::now());
    }

    /// Run the autoscaling loop
    ///
    /// This method should be spawned as a background task. It will continuously
    /// evaluate scaling decisions at the configured interval until shutdown is
    /// signaled.
    ///
    /// # Returns
    /// Returns `Ok(())` when shutdown is signaled, or an error if something
    /// goes wrong.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let controller = Arc::new(AutoscaleController::new(...));
    /// let controller_clone = controller.clone();
    ///
    /// // Spawn the autoscale loop
    /// let handle = tokio::spawn(async move {
    ///     controller_clone.run_loop().await
    /// });
    ///
    /// // Later, shutdown
    /// controller.shutdown();
    /// handle.await.unwrap();
    /// ```
    pub async fn run_loop(&self) -> Result<()> {
        let mut ticker = tokio::time::interval(self.interval);

        info!(
            interval_ms = self.interval.as_millis() as u64,
            "Starting autoscale controller loop"
        );

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    self.evaluate_all_services().await;
                }
                _ = self.shutdown.notified() => {
                    info!("Autoscale controller shutting down");
                    break;
                }
            }
        }

        Ok(())
    }

    /// Evaluate and potentially scale all registered services
    async fn evaluate_all_services(&self) {
        // Get list of registered services
        let service_names: Vec<String> = {
            let specs = self.service_specs.read().await;
            specs.keys().cloned().collect()
        };

        for service_name in service_names {
            if let Err(e) = self.evaluate_and_scale(&service_name).await {
                // Log but don't fail the entire loop
                warn!(
                    service = %service_name,
                    error = %e,
                    "Failed to evaluate/scale service"
                );
            }
        }
    }

    /// Evaluate a single service and execute scaling if needed
    async fn evaluate_and_scale(&self, service_name: &str) -> Result<()> {
        // Check cooldown first
        if !self.should_scale(service_name).await {
            return Ok(());
        }

        // Collect metrics
        let aggregated = match self.metrics.collect(service_name).await {
            Ok(m) => m,
            Err(e) => {
                // Missing metrics is not necessarily an error - the service might
                // not have any running containers yet
                debug!(
                    service = service_name,
                    error = %e,
                    "No metrics available for service"
                );
                return Ok(());
            }
        };

        // Make scaling decision
        let decision = {
            let mut autoscaler = self.autoscaler.write().await;
            match autoscaler.evaluate(service_name, &aggregated) {
                Ok(d) => d,
                Err(e) => {
                    debug!(
                        service = service_name,
                        error = %e,
                        "Failed to evaluate scaling"
                    );
                    return Ok(());
                }
            }
        };

        debug!(
            service = service_name,
            ?decision,
            cpu = aggregated.avg_cpu_percent,
            memory = aggregated.avg_memory_percent,
            instances = aggregated.instance_count,
            "Autoscale evaluation"
        );

        // Execute scaling if needed
        if let Some(target) = decision.target_replicas() {
            info!(
                service = service_name,
                target_replicas = target,
                decision = ?decision,
                "Executing autoscale"
            );

            // Execute the scaling
            if let Err(e) = self
                .service_manager
                .scale_service(service_name, target)
                .await
            {
                error!(
                    service = service_name,
                    target = target,
                    error = %e,
                    "Failed to scale service"
                );
                return Err(e);
            }

            // Record the scale action
            self.record_scale_action(service_name).await;

            // Update the autoscaler's internal state
            {
                let mut autoscaler = self.autoscaler.write().await;
                if let Err(e) = autoscaler.record_scale_action(service_name, target) {
                    warn!(
                        service = service_name,
                        error = %e,
                        "Failed to record scale action in autoscaler"
                    );
                }
            }
        }

        Ok(())
    }

    /// Signal shutdown of the autoscale loop
    pub fn shutdown(&self) {
        self.shutdown.notify_one();
    }

    /// Get the current evaluation interval
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Get registered service count
    pub async fn registered_service_count(&self) -> usize {
        let specs = self.service_specs.read().await;
        specs.len()
    }
}

/// Check if any service in a deployment has adaptive scaling
///
/// This is a helper function to determine if the autoscale controller should
/// be started for a deployment.
pub fn has_adaptive_scaling(services: &HashMap<String, zlayer_spec::ServiceSpec>) -> bool {
    services
        .values()
        .any(|s| matches!(s.scale, ScaleSpec::Adaptive { .. }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MockRuntime;
    use zlayer_scheduler::metrics::{MockMetricsSource, ServiceMetrics};
    use zlayer_spec::ScaleTargets;

    fn mock_spec() -> zlayer_spec::ServiceSpec {
        serde_yaml::from_str::<zlayer_spec::DeploymentSpec>(
            r#"
version: v1
deployment: test
services:
  test:
    rtype: service
    image:
      name: test:latest
    endpoints:
      - name: http
        protocol: http
        port: 8080
    scale:
      mode: fixed
      replicas: 1
"#,
        )
        .unwrap()
        .services
        .remove("test")
        .unwrap()
    }

    fn adaptive_spec(
        min: u32,
        max: u32,
        cpu_target: Option<u8>,
        memory_target: Option<u8>,
    ) -> ScaleSpec {
        ScaleSpec::Adaptive {
            min,
            max,
            cooldown: Some(Duration::from_secs(0)), // No cooldown for tests
            targets: ScaleTargets {
                cpu: cpu_target,
                memory: memory_target,
                rps: None,
            },
        }
    }

    #[tokio::test]
    async fn test_autoscale_controller_creation() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = Arc::new(ServiceManager::new(runtime.clone()));

        let controller = AutoscaleController::new(manager, runtime, Duration::from_secs(10));

        assert_eq!(controller.interval(), Duration::from_secs(10));
        assert_eq!(controller.registered_service_count().await, 0);
    }

    #[tokio::test]
    async fn test_register_service() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = Arc::new(ServiceManager::new(runtime.clone()));

        let controller = AutoscaleController::new(manager, runtime, Duration::from_secs(10));

        // Register an adaptive service
        let spec = adaptive_spec(1, 10, Some(70), None);
        controller.register_service("api", &spec, 2).await;

        assert!(controller.is_registered("api").await);
        assert_eq!(controller.registered_service_count().await, 1);
    }

    #[tokio::test]
    async fn test_register_fixed_service_ignored() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = Arc::new(ServiceManager::new(runtime.clone()));

        let controller = AutoscaleController::new(manager, runtime, Duration::from_secs(10));

        // Try to register a fixed service - should be ignored
        let spec = ScaleSpec::Fixed { replicas: 3 };
        controller.register_service("api", &spec, 3).await;

        assert!(!controller.is_registered("api").await);
        assert_eq!(controller.registered_service_count().await, 0);
    }

    #[tokio::test]
    async fn test_unregister_service() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = Arc::new(ServiceManager::new(runtime.clone()));

        let controller = AutoscaleController::new(manager, runtime, Duration::from_secs(10));

        let spec = adaptive_spec(1, 10, Some(70), None);
        controller.register_service("api", &spec, 2).await;

        assert!(controller.is_registered("api").await);

        controller.unregister_service("api").await;

        assert!(!controller.is_registered("api").await);
        assert_eq!(controller.registered_service_count().await, 0);
    }

    #[tokio::test]
    async fn test_has_adaptive_scaling() {
        let mut services = HashMap::new();

        // Add a fixed service
        let mut fixed_spec = mock_spec();
        fixed_spec.scale = ScaleSpec::Fixed { replicas: 3 };
        services.insert("web".to_string(), fixed_spec);

        // No adaptive services yet
        assert!(!has_adaptive_scaling(&services));

        // Add an adaptive service
        let mut adaptive = mock_spec();
        adaptive.scale = adaptive_spec(1, 10, Some(70), None);
        services.insert("api".to_string(), adaptive);

        // Now has adaptive
        assert!(has_adaptive_scaling(&services));
    }

    #[tokio::test]
    async fn test_autoscale_controller_with_mock_metrics() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = Arc::new(ServiceManager::new(runtime.clone()));

        // Create mock metrics source
        let mock = Arc::new(MockMetricsSource::new());

        // Set high CPU metrics
        mock.set_metrics(
            "api",
            vec![
                ServiceMetrics {
                    cpu_percent: 85.0,
                    memory_bytes: 100 * 1024 * 1024,
                    memory_limit: 512 * 1024 * 1024,
                    rps: None,
                    timestamp: Some(Instant::now()),
                },
                ServiceMetrics {
                    cpu_percent: 90.0,
                    memory_bytes: 150 * 1024 * 1024,
                    memory_limit: 512 * 1024 * 1024,
                    rps: None,
                    timestamp: Some(Instant::now()),
                },
            ],
        )
        .await;

        // Create controller with custom metrics
        let mut metrics = MetricsCollector::new();
        metrics.add_source(mock);

        let controller = AutoscaleController::with_custom_metrics(
            manager.clone(),
            metrics,
            Duration::from_secs(10),
        );

        // Register service
        manager
            .upsert_service("api".to_string(), mock_spec())
            .await
            .unwrap();
        manager.scale_service("api", 2).await.unwrap();

        let spec = adaptive_spec(1, 10, Some(70), None);
        controller.register_service("api", &spec, 2).await;

        // Evaluate - should want to scale up due to high CPU
        controller.evaluate_and_scale("api").await.unwrap();

        // Check that scale happened (from 2 to 3)
        let count = manager.service_replica_count("api").await.unwrap();
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn test_autoscale_controller_cooldown() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = Arc::new(ServiceManager::new(runtime.clone()));

        let controller = AutoscaleController::new(manager, runtime, Duration::from_secs(10));

        // Use a spec with 1 second cooldown
        let spec = ScaleSpec::Adaptive {
            min: 1,
            max: 10,
            cooldown: Some(Duration::from_secs(60)), // Long cooldown
            targets: ScaleTargets {
                cpu: Some(70),
                memory: None,
                rps: None,
            },
        };

        controller.register_service("api", &spec, 2).await;

        // Initially should be able to scale
        assert!(controller.should_scale("api").await);

        // Record a scale action
        controller.record_scale_action("api").await;

        // Now should be in cooldown
        assert!(!controller.should_scale("api").await);
    }

    #[tokio::test]
    async fn test_autoscale_controller_shutdown() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = Arc::new(ServiceManager::new(runtime.clone()));

        let controller = Arc::new(AutoscaleController::new(
            manager,
            runtime,
            Duration::from_millis(100), // Fast interval for test
        ));

        let controller_clone = controller.clone();

        // Spawn the loop
        let handle = tokio::spawn(async move { controller_clone.run_loop().await });

        // Let it run briefly
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Signal shutdown
        controller.shutdown();

        // Should complete without error
        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }
}
