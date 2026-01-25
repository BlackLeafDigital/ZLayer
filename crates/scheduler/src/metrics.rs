//! Metrics collection for autoscaling decisions
//!
//! Provides CPU, memory, and RPS metrics from container runtimes.
//!
//! # Architecture
//!
//! This module provides a trait-based interface for metrics collection that can
//! be implemented by different backend sources (cgroups, containerd, mock, etc.).
//!
//! The primary implementation is `CgroupsMetricsSource` which collects metrics
//! from Linux cgroups v2 via the agent crate's runtime abstraction.
//!
//! # Example
//!
//! ```ignore
//! use scheduler::metrics::{MetricsCollector, CgroupsMetricsSource};
//! use std::sync::Arc;
//!
//! // Create a metrics source backed by agent runtime
//! let service_provider = /* ... */;
//! let stats_provider = /* ... */;
//! let source = CgroupsMetricsSource::new(service_provider, stats_provider);
//!
//! // Add to collector
//! let mut collector = MetricsCollector::new();
//! collector.add_source(Arc::new(source));
//!
//! // Collect metrics
//! let metrics = collector.collect("my-service").await?;
//! ```

use async_trait::async_trait;
use prometheus::{GaugeVec, Opts, Registry};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::RwLock;
use tracing::{debug, warn};

use crate::error::{Result, SchedulerError};

/// Metrics for a single service instance
#[derive(Debug, Clone, Default)]
pub struct ServiceMetrics {
    /// CPU usage as percentage (0.0-100.0)
    pub cpu_percent: f64,
    /// Memory usage in bytes
    pub memory_bytes: u64,
    /// Memory limit in bytes
    pub memory_limit: u64,
    /// Requests per second (if available)
    pub rps: Option<f64>,
    /// Timestamp when metrics were collected
    pub timestamp: Option<Instant>,
}

impl ServiceMetrics {
    /// Calculate memory usage as percentage
    pub fn memory_percent(&self) -> f64 {
        if self.memory_limit > 0 {
            (self.memory_bytes as f64 / self.memory_limit as f64) * 100.0
        } else {
            0.0
        }
    }

    /// Check if metrics are stale (older than given duration)
    pub fn is_stale(&self, max_age: Duration) -> bool {
        self.timestamp
            .map(|ts| ts.elapsed() > max_age)
            .unwrap_or(true)
    }
}

/// Aggregated metrics across all instances of a service
#[derive(Debug, Clone, Default)]
pub struct AggregatedMetrics {
    /// Average CPU usage across instances
    pub avg_cpu_percent: f64,
    /// Average memory usage percentage
    pub avg_memory_percent: f64,
    /// Total RPS across all instances
    pub total_rps: Option<f64>,
    /// Number of instances reporting metrics
    pub instance_count: usize,
}

/// Source of metrics data
#[async_trait]
pub trait MetricsSource: Send + Sync {
    /// Collect metrics for all instances of a service
    async fn collect(&self, service_name: &str) -> Result<Vec<ServiceMetrics>>;

    /// Get the name of this metrics source
    fn name(&self) -> &'static str;
}

/// Metrics collector that aggregates data from multiple sources
pub struct MetricsCollector {
    sources: Vec<Arc<dyn MetricsSource>>,
    cache: Arc<RwLock<HashMap<String, (AggregatedMetrics, Instant)>>>,
    cache_ttl: Duration,
    registry: Registry,
    // Prometheus gauges
    cpu_gauge: GaugeVec,
    memory_gauge: GaugeVec,
    rps_gauge: GaugeVec,
    instance_gauge: GaugeVec,
}

impl MetricsCollector {
    /// Create a new metrics collector
    pub fn new() -> Self {
        Self::with_cache_ttl(Duration::from_secs(5))
    }

    /// Create with custom cache TTL
    pub fn with_cache_ttl(cache_ttl: Duration) -> Self {
        let registry = Registry::new();

        let cpu_gauge = GaugeVec::new(
            Opts::new("zlayer_service_cpu_percent", "Average CPU usage percentage"),
            &["service"],
        )
        .expect("metric creation failed");

        let memory_gauge = GaugeVec::new(
            Opts::new(
                "zlayer_service_memory_percent",
                "Average memory usage percentage",
            ),
            &["service"],
        )
        .expect("metric creation failed");

        let rps_gauge = GaugeVec::new(
            Opts::new("zlayer_service_rps", "Requests per second"),
            &["service"],
        )
        .expect("metric creation failed");

        let instance_gauge = GaugeVec::new(
            Opts::new("zlayer_service_instances", "Number of service instances"),
            &["service"],
        )
        .expect("metric creation failed");

        registry.register(Box::new(cpu_gauge.clone())).ok();
        registry.register(Box::new(memory_gauge.clone())).ok();
        registry.register(Box::new(rps_gauge.clone())).ok();
        registry.register(Box::new(instance_gauge.clone())).ok();

        Self {
            sources: Vec::new(),
            cache: Arc::new(RwLock::new(HashMap::new())),
            cache_ttl,
            registry,
            cpu_gauge,
            memory_gauge,
            rps_gauge,
            instance_gauge,
        }
    }

    /// Add a metrics source
    pub fn add_source(&mut self, source: Arc<dyn MetricsSource>) {
        self.sources.push(source);
    }

    /// Get the Prometheus registry for exposition
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Collect and aggregate metrics for a service
    pub async fn collect(&self, service_name: &str) -> Result<AggregatedMetrics> {
        // Check cache first
        {
            let cache = self.cache.read().await;
            if let Some((metrics, timestamp)) = cache.get(service_name) {
                if timestamp.elapsed() < self.cache_ttl {
                    debug!(service = service_name, "Returning cached metrics");
                    return Ok(metrics.clone());
                }
            }
        }

        // Collect from all sources
        let mut all_metrics = Vec::new();
        for source in &self.sources {
            match source.collect(service_name).await {
                Ok(metrics) => {
                    debug!(
                        service = service_name,
                        source = source.name(),
                        count = metrics.len(),
                        "Collected metrics"
                    );
                    all_metrics.extend(metrics);
                }
                Err(e) => {
                    warn!(
                        service = service_name,
                        source = source.name(),
                        error = %e,
                        "Failed to collect metrics"
                    );
                }
            }
        }

        if all_metrics.is_empty() {
            return Err(SchedulerError::MetricsCollection(format!(
                "No metrics available for service: {}",
                service_name
            )));
        }

        // Aggregate
        let aggregated = Self::aggregate(&all_metrics);

        // Update Prometheus gauges
        self.cpu_gauge
            .with_label_values(&[service_name])
            .set(aggregated.avg_cpu_percent);
        self.memory_gauge
            .with_label_values(&[service_name])
            .set(aggregated.avg_memory_percent);
        if let Some(rps) = aggregated.total_rps {
            self.rps_gauge.with_label_values(&[service_name]).set(rps);
        }
        self.instance_gauge
            .with_label_values(&[service_name])
            .set(aggregated.instance_count as f64);

        // Update cache
        {
            let mut cache = self.cache.write().await;
            cache.insert(
                service_name.to_string(),
                (aggregated.clone(), Instant::now()),
            );
        }

        Ok(aggregated)
    }

    /// Aggregate metrics from multiple instances
    fn aggregate(metrics: &[ServiceMetrics]) -> AggregatedMetrics {
        if metrics.is_empty() {
            return AggregatedMetrics::default();
        }

        let instance_count = metrics.len();
        let total_cpu: f64 = metrics.iter().map(|m| m.cpu_percent).sum();
        let total_memory_percent: f64 = metrics.iter().map(|m| m.memory_percent()).sum();
        let total_rps: Option<f64> = {
            let rps_values: Vec<f64> = metrics.iter().filter_map(|m| m.rps).collect();
            if rps_values.is_empty() {
                None
            } else {
                Some(rps_values.iter().sum())
            }
        };

        AggregatedMetrics {
            avg_cpu_percent: total_cpu / instance_count as f64,
            avg_memory_percent: total_memory_percent / instance_count as f64,
            total_rps,
            instance_count,
        }
    }

    /// Clear cached metrics for a service
    pub async fn invalidate_cache(&self, service_name: &str) {
        let mut cache = self.cache.write().await;
        cache.remove(service_name);
    }

    /// Clear all cached metrics
    pub async fn clear_cache(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Mock metrics source for testing
pub struct MockMetricsSource {
    metrics: Arc<RwLock<HashMap<String, Vec<ServiceMetrics>>>>,
}

impl MockMetricsSource {
    /// Create a new mock source
    pub fn new() -> Self {
        Self {
            metrics: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Set metrics for a service (for testing)
    pub async fn set_metrics(&self, service_name: &str, metrics: Vec<ServiceMetrics>) {
        let mut map = self.metrics.write().await;
        map.insert(service_name.to_string(), metrics);
    }

    /// Clear all metrics
    pub async fn clear(&self) {
        let mut map = self.metrics.write().await;
        map.clear();
    }
}

impl Default for MockMetricsSource {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MetricsSource for MockMetricsSource {
    async fn collect(&self, service_name: &str) -> Result<Vec<ServiceMetrics>> {
        let map = self.metrics.read().await;
        map.get(service_name).cloned().ok_or_else(|| {
            SchedulerError::MetricsCollection(format!(
                "No mock metrics for service: {}",
                service_name
            ))
        })
    }

    fn name(&self) -> &'static str {
        "mock"
    }
}

/// Containerd-based metrics source (for production)
///
/// Collects CPU and memory stats from containerd container runtime.
pub struct ContainerdMetricsSource {
    socket_path: String,
    namespace: String,
}

impl ContainerdMetricsSource {
    /// Create a new containerd metrics source
    pub fn new(socket_path: impl Into<String>, namespace: impl Into<String>) -> Self {
        Self {
            socket_path: socket_path.into(),
            namespace: namespace.into(),
        }
    }

    /// Default socket path for containerd
    pub fn default_socket() -> &'static str {
        "/run/containerd/containerd.sock"
    }

    /// Default namespace
    pub fn default_namespace() -> &'static str {
        "zlayer"
    }

    /// Create with defaults
    pub fn with_defaults() -> Self {
        Self::new(Self::default_socket(), Self::default_namespace())
    }
}

#[async_trait]
impl MetricsSource for ContainerdMetricsSource {
    async fn collect(&self, service_name: &str) -> Result<Vec<ServiceMetrics>> {
        // Note: Full containerd integration requires:
        // 1. Connect to containerd gRPC socket
        // 2. List containers matching service_name label
        // 3. Get metrics for each container
        //
        // For now, provide a stub that can be fleshed out.
        // Real implementation would use containerd-client crate.

        debug!(
            service = service_name,
            socket = %self.socket_path,
            namespace = %self.namespace,
            "Collecting containerd metrics (stub)"
        );

        // TODO: Implement actual containerd metrics collection
        // This requires:
        // - containerd_client::connect()
        // - task service to get metrics
        // - parsing cgroup stats

        // For now, return empty to indicate no containers found
        // This allows the system to work without containerd running
        Ok(Vec::new())
    }

    fn name(&self) -> &'static str {
        "containerd"
    }
}

// ============================================================================
// Cgroups-based Metrics Source
// ============================================================================

/// Container identifier for metrics collection
///
/// This is a simplified identifier that matches the agent crate's ContainerId
/// but avoids a direct dependency.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MetricsContainerId {
    /// Service name this container belongs to
    pub service: String,
    /// Replica number (1-indexed)
    pub replica: u32,
}

impl std::fmt::Display for MetricsContainerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-{}", self.service, self.replica)
    }
}

/// Raw container statistics from cgroups
///
/// This mirrors the agent crate's ContainerStats structure but allows
/// the scheduler to work without a direct dependency on agent.
#[derive(Debug, Clone)]
pub struct RawContainerStats {
    /// CPU usage in microseconds (cumulative)
    pub cpu_usage_usec: u64,
    /// Current memory usage in bytes
    pub memory_bytes: u64,
    /// Memory limit in bytes (u64::MAX if unlimited)
    pub memory_limit: u64,
    /// Timestamp when stats were collected
    pub timestamp: Instant,
}

impl RawContainerStats {
    /// Calculate memory usage as a percentage
    pub fn memory_percent(&self) -> f64 {
        if self.memory_limit == u64::MAX || self.memory_limit == 0 {
            0.0
        } else {
            (self.memory_bytes as f64 / self.memory_limit as f64) * 100.0
        }
    }
}

/// Trait for providing the list of containers for a service
///
/// Implementations of this trait provide the mapping from service names
/// to container identifiers. This is typically implemented by the agent's
/// ServiceManager.
#[async_trait]
pub trait ServiceContainerProvider: Send + Sync {
    /// Get all container IDs for a given service
    async fn get_container_ids(&self, service_name: &str) -> Vec<MetricsContainerId>;

    /// Get all services and their containers
    async fn get_all_services(&self) -> HashMap<String, Vec<MetricsContainerId>>;
}

/// Trait for providing container resource statistics
///
/// Implementations of this trait fetch raw cgroup stats for containers.
/// This is typically implemented by the agent's Runtime.
#[async_trait]
pub trait ContainerStatsProvider: Send + Sync {
    /// Get raw container statistics
    async fn get_stats(
        &self,
        id: &MetricsContainerId,
    ) -> std::result::Result<RawContainerStats, String>;
}

/// Cgroups-based metrics source for production use
///
/// Collects CPU and memory statistics from Linux cgroups v2 via
/// the agent runtime abstraction. Calculates CPU percentage by
/// comparing consecutive samples.
///
/// # CPU Calculation
///
/// CPU percentage is calculated using the formula:
/// ```text
/// cpu_percent = (delta_usage_usec / (delta_time_usec * num_cpus)) * 100
/// ```
///
/// This requires storing previous stats to compute deltas.
///
/// # Memory Calculation
///
/// Memory percentage is straightforward:
/// ```text
/// memory_percent = (memory_bytes / memory_limit) * 100
/// ```
pub struct CgroupsMetricsSource<S, P>
where
    S: ServiceContainerProvider,
    P: ContainerStatsProvider,
{
    /// Provider for service-to-container mapping
    service_provider: Arc<S>,
    /// Provider for container statistics
    stats_provider: Arc<P>,
    /// Previous stats for CPU delta calculation (container_id -> stats)
    prev_stats: RwLock<HashMap<String, RawContainerStats>>,
    /// Number of CPUs for CPU percentage calculation
    num_cpus: u64,
}

impl<S, P> CgroupsMetricsSource<S, P>
where
    S: ServiceContainerProvider,
    P: ContainerStatsProvider,
{
    /// Create a new cgroups metrics source
    ///
    /// # Arguments
    /// * `service_provider` - Provider for service-to-container mapping
    /// * `stats_provider` - Provider for container statistics
    pub fn new(service_provider: Arc<S>, stats_provider: Arc<P>) -> Self {
        Self {
            service_provider,
            stats_provider,
            prev_stats: RwLock::new(HashMap::new()),
            num_cpus: num_cpus::get() as u64,
        }
    }

    /// Create with a specific CPU count (for testing or container-limited scenarios)
    pub fn with_cpus(service_provider: Arc<S>, stats_provider: Arc<P>, num_cpus: u64) -> Self {
        Self {
            service_provider,
            stats_provider,
            prev_stats: RwLock::new(HashMap::new()),
            num_cpus,
        }
    }

    /// Calculate CPU percentage from two consecutive samples
    ///
    /// Uses the formula: (delta_usage / (delta_time * num_cpus)) * 100
    fn calculate_cpu_percent(&self, prev: &RawContainerStats, curr: &RawContainerStats) -> f64 {
        let usage_delta = curr.cpu_usage_usec.saturating_sub(prev.cpu_usage_usec);
        let time_delta = curr.timestamp.duration_since(prev.timestamp);
        let time_delta_usec = time_delta.as_micros() as u64;

        if time_delta_usec == 0 || self.num_cpus == 0 {
            return 0.0;
        }

        (usage_delta as f64 / (time_delta_usec * self.num_cpus) as f64) * 100.0
    }

    /// Collect metrics for a single container
    async fn collect_container_metrics(&self, id: &MetricsContainerId) -> Option<ServiceMetrics> {
        let id_str = id.to_string();

        // Get current stats
        let curr_stats = match self.stats_provider.get_stats(id).await {
            Ok(stats) => stats,
            Err(e) => {
                debug!(
                    container = %id_str,
                    error = %e,
                    "Failed to get container stats"
                );
                return None;
            }
        };

        // Calculate CPU percentage from previous sample
        let cpu_percent = {
            let prev_stats = self.prev_stats.read().await;
            if let Some(prev) = prev_stats.get(&id_str) {
                self.calculate_cpu_percent(prev, &curr_stats)
            } else {
                // No previous sample - first collection, return 0
                0.0
            }
        };

        // Store current stats for next calculation
        {
            let mut prev_stats = self.prev_stats.write().await;
            prev_stats.insert(id_str, curr_stats.clone());
        }

        Some(ServiceMetrics {
            cpu_percent,
            memory_bytes: curr_stats.memory_bytes,
            memory_limit: curr_stats.memory_limit,
            rps: None, // RPS requires separate instrumentation (e.g., from proxy)
            timestamp: Some(curr_stats.timestamp),
        })
    }

    /// Clear cached previous stats (useful for testing or when containers restart)
    pub async fn clear_cache(&self) {
        let mut prev_stats = self.prev_stats.write().await;
        prev_stats.clear();
    }

    /// Remove cached stats for a specific container
    pub async fn remove_container_cache(&self, id: &MetricsContainerId) {
        let mut prev_stats = self.prev_stats.write().await;
        prev_stats.remove(&id.to_string());
    }
}

#[async_trait]
impl<S, P> MetricsSource for CgroupsMetricsSource<S, P>
where
    S: ServiceContainerProvider + 'static,
    P: ContainerStatsProvider + 'static,
{
    async fn collect(&self, service_name: &str) -> Result<Vec<ServiceMetrics>> {
        // Get all container IDs for this service
        let container_ids = self.service_provider.get_container_ids(service_name).await;

        if container_ids.is_empty() {
            debug!(service = service_name, "No containers found for service");
            return Err(SchedulerError::MetricsCollection(format!(
                "No containers found for service: {}",
                service_name
            )));
        }

        // Collect metrics from all containers
        let mut metrics = Vec::with_capacity(container_ids.len());
        for id in &container_ids {
            if let Some(m) = self.collect_container_metrics(id).await {
                metrics.push(m);
            }
        }

        if metrics.is_empty() {
            return Err(SchedulerError::MetricsCollection(format!(
                "Failed to collect metrics from any container for service: {}",
                service_name
            )));
        }

        debug!(
            service = service_name,
            container_count = container_ids.len(),
            metrics_collected = metrics.len(),
            "Collected cgroups metrics"
        );

        Ok(metrics)
    }

    fn name(&self) -> &'static str {
        "cgroups"
    }
}

// ============================================================================
// Aggregate Metrics Collection (for autoscaler)
// ============================================================================

/// Service-level aggregated metrics for autoscaling
///
/// This struct contains the metrics needed for autoscaling decisions,
/// aggregated across all replicas of a service.
#[derive(Debug, Clone)]
pub struct AutoscaleMetrics {
    /// Service name
    pub service: String,
    /// Average CPU usage percentage across replicas
    pub cpu_percent: f64,
    /// Average memory usage percentage across replicas
    pub memory_percent: f64,
    /// Requests per second (if available from proxy metrics)
    pub rps: Option<f64>,
    /// Number of replicas currently running
    pub replica_count: usize,
    /// Timestamp when metrics were collected
    pub timestamp: SystemTime,
}

/// Collect all service metrics for autoscaling decisions
///
/// This is a convenience function that uses a CgroupsMetricsSource to
/// collect and aggregate metrics for all services.
pub async fn collect_all_service_metrics<S, P>(
    source: &CgroupsMetricsSource<S, P>,
) -> Vec<AutoscaleMetrics>
where
    S: ServiceContainerProvider + 'static,
    P: ContainerStatsProvider + 'static,
{
    let all_services = source.service_provider.get_all_services().await;
    let mut results = Vec::with_capacity(all_services.len());

    for (service_name, containers) in all_services {
        if containers.is_empty() {
            continue;
        }

        // Collect metrics for this service
        let mut cpu_samples = Vec::new();
        let mut memory_samples = Vec::new();
        let mut rps_sum = 0.0;
        let mut has_rps = false;

        for container_id in &containers {
            if let Some(metrics) = source.collect_container_metrics(container_id).await {
                cpu_samples.push(metrics.cpu_percent);
                memory_samples.push(metrics.memory_percent());

                if let Some(rps) = metrics.rps {
                    rps_sum += rps;
                    has_rps = true;
                }
            }
        }

        if !cpu_samples.is_empty() {
            results.push(AutoscaleMetrics {
                service: service_name,
                cpu_percent: cpu_samples.iter().sum::<f64>() / cpu_samples.len() as f64,
                memory_percent: memory_samples.iter().sum::<f64>() / memory_samples.len() as f64,
                rps: if has_rps { Some(rps_sum) } else { None },
                replica_count: containers.len(),
                timestamp: SystemTime::now(),
            });
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_metrics_memory_percent() {
        let metrics = ServiceMetrics {
            cpu_percent: 50.0,
            memory_bytes: 512 * 1024 * 1024,  // 512MB
            memory_limit: 1024 * 1024 * 1024, // 1GB
            rps: None,
            timestamp: Some(Instant::now()),
        };

        assert!((metrics.memory_percent() - 50.0).abs() < 0.001);
    }

    #[test]
    fn test_aggregate_metrics() {
        let metrics = vec![
            ServiceMetrics {
                cpu_percent: 40.0,
                memory_bytes: 400,
                memory_limit: 1000,
                rps: Some(100.0),
                timestamp: Some(Instant::now()),
            },
            ServiceMetrics {
                cpu_percent: 60.0,
                memory_bytes: 600,
                memory_limit: 1000,
                rps: Some(200.0),
                timestamp: Some(Instant::now()),
            },
        ];

        let aggregated = MetricsCollector::aggregate(&metrics);

        assert!((aggregated.avg_cpu_percent - 50.0).abs() < 0.001);
        assert!((aggregated.avg_memory_percent - 50.0).abs() < 0.001);
        assert_eq!(aggregated.total_rps, Some(300.0));
        assert_eq!(aggregated.instance_count, 2);
    }

    #[tokio::test]
    async fn test_mock_metrics_source() {
        let source = MockMetricsSource::new();

        // Initially no metrics
        let result = source.collect("test-service").await;
        assert!(result.is_err());

        // Set some metrics
        source
            .set_metrics(
                "test-service",
                vec![ServiceMetrics {
                    cpu_percent: 45.0,
                    memory_bytes: 256 * 1024 * 1024,
                    memory_limit: 512 * 1024 * 1024,
                    rps: Some(150.0),
                    timestamp: Some(Instant::now()),
                }],
            )
            .await;

        // Now should succeed
        let metrics = source.collect("test-service").await.unwrap();
        assert_eq!(metrics.len(), 1);
        assert!((metrics[0].cpu_percent - 45.0).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_metrics_collector_with_mock() {
        let mock = Arc::new(MockMetricsSource::new());
        mock.set_metrics(
            "api",
            vec![
                ServiceMetrics {
                    cpu_percent: 30.0,
                    memory_bytes: 100,
                    memory_limit: 200,
                    rps: Some(50.0),
                    timestamp: Some(Instant::now()),
                },
                ServiceMetrics {
                    cpu_percent: 50.0,
                    memory_bytes: 150,
                    memory_limit: 200,
                    rps: Some(75.0),
                    timestamp: Some(Instant::now()),
                },
            ],
        )
        .await;

        let mut collector = MetricsCollector::new();
        collector.add_source(mock);

        let aggregated = collector.collect("api").await.unwrap();
        assert_eq!(aggregated.instance_count, 2);
        assert!((aggregated.avg_cpu_percent - 40.0).abs() < 0.001);
        assert_eq!(aggregated.total_rps, Some(125.0));
    }

    // ========================================================================
    // CgroupsMetricsSource Tests
    // ========================================================================

    /// Mock service container provider for testing
    struct MockServiceProvider {
        containers: RwLock<HashMap<String, Vec<MetricsContainerId>>>,
    }

    impl MockServiceProvider {
        fn new() -> Self {
            Self {
                containers: RwLock::new(HashMap::new()),
            }
        }

        async fn add_service(&self, service: &str, replica_count: u32) {
            let ids: Vec<MetricsContainerId> = (1..=replica_count)
                .map(|i| MetricsContainerId {
                    service: service.to_string(),
                    replica: i,
                })
                .collect();

            let mut containers = self.containers.write().await;
            containers.insert(service.to_string(), ids);
        }
    }

    #[async_trait]
    impl ServiceContainerProvider for MockServiceProvider {
        async fn get_container_ids(&self, service_name: &str) -> Vec<MetricsContainerId> {
            let containers = self.containers.read().await;
            containers.get(service_name).cloned().unwrap_or_default()
        }

        async fn get_all_services(&self) -> HashMap<String, Vec<MetricsContainerId>> {
            self.containers.read().await.clone()
        }
    }

    /// Mock stats provider for testing
    struct MockStatsProvider {
        stats: RwLock<HashMap<String, RawContainerStats>>,
    }

    impl MockStatsProvider {
        fn new() -> Self {
            Self {
                stats: RwLock::new(HashMap::new()),
            }
        }

        async fn set_stats(&self, id: &MetricsContainerId, stats: RawContainerStats) {
            let mut map = self.stats.write().await;
            map.insert(id.to_string(), stats);
        }
    }

    #[async_trait]
    impl ContainerStatsProvider for MockStatsProvider {
        async fn get_stats(
            &self,
            id: &MetricsContainerId,
        ) -> std::result::Result<RawContainerStats, String> {
            let map = self.stats.read().await;
            map.get(&id.to_string())
                .cloned()
                .ok_or_else(|| format!("No stats for container: {}", id))
        }
    }

    #[test]
    fn test_metrics_container_id_display() {
        let id = MetricsContainerId {
            service: "web".to_string(),
            replica: 3,
        };
        assert_eq!(id.to_string(), "web-3");
    }

    #[test]
    fn test_raw_container_stats_memory_percent() {
        let stats = RawContainerStats {
            cpu_usage_usec: 1_000_000,
            memory_bytes: 512 * 1024 * 1024,
            memory_limit: 1024 * 1024 * 1024,
            timestamp: Instant::now(),
        };
        assert!((stats.memory_percent() - 50.0).abs() < 0.001);
    }

    #[test]
    fn test_raw_container_stats_memory_percent_unlimited() {
        let stats = RawContainerStats {
            cpu_usage_usec: 1_000_000,
            memory_bytes: 512 * 1024 * 1024,
            memory_limit: u64::MAX,
            timestamp: Instant::now(),
        };
        assert_eq!(stats.memory_percent(), 0.0);
    }

    #[tokio::test]
    async fn test_cgroups_metrics_source_no_containers() {
        let service_provider = Arc::new(MockServiceProvider::new());
        let stats_provider = Arc::new(MockStatsProvider::new());

        let source = CgroupsMetricsSource::with_cpus(service_provider, stats_provider, 4);

        // No containers registered - should return error
        let result = source.collect("unknown-service").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cgroups_metrics_source_first_sample() {
        let service_provider = Arc::new(MockServiceProvider::new());
        let stats_provider = Arc::new(MockStatsProvider::new());

        // Add a service with 1 replica
        service_provider.add_service("api", 1).await;

        // Set stats for the container
        let container_id = MetricsContainerId {
            service: "api".to_string(),
            replica: 1,
        };
        stats_provider
            .set_stats(
                &container_id,
                RawContainerStats {
                    cpu_usage_usec: 1_000_000,
                    memory_bytes: 50 * 1024 * 1024,
                    memory_limit: 256 * 1024 * 1024,
                    timestamp: Instant::now(),
                },
            )
            .await;

        let source = CgroupsMetricsSource::with_cpus(
            service_provider,
            stats_provider,
            4, // 4 CPUs
        );

        // First sample - CPU should be 0 (no previous sample)
        let metrics = source.collect("api").await.unwrap();
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].cpu_percent, 0.0); // First sample has no delta
        assert!((metrics[0].memory_percent() - 19.53).abs() < 0.1); // ~50MB/256MB
    }

    #[tokio::test]
    async fn test_cgroups_metrics_source_cpu_calculation() {
        let service_provider = Arc::new(MockServiceProvider::new());
        let stats_provider = Arc::new(MockStatsProvider::new());

        // Add a service with 1 replica
        service_provider.add_service("worker", 1).await;

        let container_id = MetricsContainerId {
            service: "worker".to_string(),
            replica: 1,
        };

        let source = CgroupsMetricsSource::with_cpus(
            service_provider.clone(),
            stats_provider.clone(),
            1, // 1 CPU for simpler calculation
        );

        // First sample: 1 second of CPU time
        let now = Instant::now();
        stats_provider
            .set_stats(
                &container_id,
                RawContainerStats {
                    cpu_usage_usec: 1_000_000, // 1 second
                    memory_bytes: 100 * 1024 * 1024,
                    memory_limit: 200 * 1024 * 1024,
                    timestamp: now,
                },
            )
            .await;

        // Collect first sample (CPU will be 0)
        let _ = source.collect("worker").await.unwrap();

        // Second sample: 1.5 seconds later with 2 seconds of CPU time
        // Delta: 1 second CPU usage over 1.5 seconds = 66.67%
        let later = now + Duration::from_millis(1500);
        stats_provider
            .set_stats(
                &container_id,
                RawContainerStats {
                    cpu_usage_usec: 2_000_000, // 2 seconds total
                    memory_bytes: 150 * 1024 * 1024,
                    memory_limit: 200 * 1024 * 1024,
                    timestamp: later,
                },
            )
            .await;

        // Collect second sample - should have calculated CPU percentage
        let metrics = source.collect("worker").await.unwrap();
        assert_eq!(metrics.len(), 1);

        // CPU delta: 1_000_000 usec over 1_500_000 usec with 1 CPU
        // Expected: (1_000_000 / (1_500_000 * 1)) * 100 = 66.67%
        assert!((metrics[0].cpu_percent - 66.67).abs() < 1.0);

        // Memory: 150MB/200MB = 75%
        assert!((metrics[0].memory_percent() - 75.0).abs() < 0.1);
    }

    #[tokio::test]
    async fn test_cgroups_metrics_source_multiple_replicas() {
        let service_provider = Arc::new(MockServiceProvider::new());
        let stats_provider = Arc::new(MockStatsProvider::new());

        // Add a service with 3 replicas
        service_provider.add_service("web", 3).await;

        // Set stats for all containers
        for i in 1..=3 {
            let container_id = MetricsContainerId {
                service: "web".to_string(),
                replica: i,
            };
            stats_provider
                .set_stats(
                    &container_id,
                    RawContainerStats {
                        cpu_usage_usec: 500_000 * (i as u64),
                        memory_bytes: 50 * 1024 * 1024 * (i as u64),
                        memory_limit: 512 * 1024 * 1024,
                        timestamp: Instant::now(),
                    },
                )
                .await;
        }

        let source = CgroupsMetricsSource::with_cpus(service_provider, stats_provider, 4);

        let metrics = source.collect("web").await.unwrap();
        assert_eq!(metrics.len(), 3);
    }

    #[tokio::test]
    async fn test_collect_all_service_metrics() {
        let service_provider = Arc::new(MockServiceProvider::new());
        let stats_provider = Arc::new(MockStatsProvider::new());

        // Add multiple services
        service_provider.add_service("api", 2).await;
        service_provider.add_service("worker", 1).await;

        // Set stats
        for (service, replica_count) in [("api", 2), ("worker", 1)] {
            for i in 1..=replica_count {
                let container_id = MetricsContainerId {
                    service: service.to_string(),
                    replica: i,
                };
                stats_provider
                    .set_stats(
                        &container_id,
                        RawContainerStats {
                            cpu_usage_usec: 1_000_000,
                            memory_bytes: 100 * 1024 * 1024,
                            memory_limit: 256 * 1024 * 1024,
                            timestamp: Instant::now(),
                        },
                    )
                    .await;
            }
        }

        let source = CgroupsMetricsSource::with_cpus(service_provider, stats_provider, 4);

        let all_metrics = collect_all_service_metrics(&source).await;
        assert_eq!(all_metrics.len(), 2);

        // Find api service metrics
        let api_metrics = all_metrics.iter().find(|m| m.service == "api");
        assert!(api_metrics.is_some());
        assert_eq!(api_metrics.unwrap().replica_count, 2);

        // Find worker service metrics
        let worker_metrics = all_metrics.iter().find(|m| m.service == "worker");
        assert!(worker_metrics.is_some());
        assert_eq!(worker_metrics.unwrap().replica_count, 1);
    }

    #[tokio::test]
    async fn test_cgroups_metrics_source_cache_clear() {
        let service_provider = Arc::new(MockServiceProvider::new());
        let stats_provider = Arc::new(MockStatsProvider::new());

        service_provider.add_service("cache-test", 1).await;

        let container_id = MetricsContainerId {
            service: "cache-test".to_string(),
            replica: 1,
        };

        stats_provider
            .set_stats(
                &container_id,
                RawContainerStats {
                    cpu_usage_usec: 1_000_000,
                    memory_bytes: 100 * 1024 * 1024,
                    memory_limit: 256 * 1024 * 1024,
                    timestamp: Instant::now(),
                },
            )
            .await;

        let source = CgroupsMetricsSource::with_cpus(service_provider, stats_provider.clone(), 4);

        // First collection - stores prev stats
        let _ = source.collect("cache-test").await.unwrap();

        // Clear cache
        source.clear_cache().await;

        // Update stats with new timestamp
        stats_provider
            .set_stats(
                &container_id,
                RawContainerStats {
                    cpu_usage_usec: 2_000_000,
                    memory_bytes: 150 * 1024 * 1024,
                    memory_limit: 256 * 1024 * 1024,
                    timestamp: Instant::now(),
                },
            )
            .await;

        // After cache clear, should be like first sample again (CPU = 0)
        let metrics = source.collect("cache-test").await.unwrap();
        assert_eq!(metrics[0].cpu_percent, 0.0);
    }
}
