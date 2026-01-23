//! Metrics collection for autoscaling decisions
//!
//! Provides CPU, memory, and RPS metrics from container runtimes.

use async_trait::async_trait;
use prometheus::{GaugeVec, Opts, Registry};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
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
}
