//! Prometheus metrics exposition
//!
//! Provides a unified metrics interface and Prometheus exposition endpoint.

use prometheus::{
    Counter, CounterVec, Encoder, Gauge, GaugeVec, HistogramVec, Opts, Registry, TextEncoder,
};
use tracing::info;

use crate::config::MetricsConfig;
use crate::error::{ObservabilityError, Result};

/// ZLayer metrics collection
///
/// Pre-defined metrics for the ZLayer system.
pub struct ZLayerMetrics {
    registry: Registry,

    // Service metrics
    /// Total number of registered services
    pub services_total: Gauge,
    /// Current replica count per service
    pub service_replicas: GaugeVec,
    /// Service health status (0=unknown, 1=healthy, 2=degraded, 3=unhealthy)
    pub service_health: GaugeVec,

    // Scaling metrics
    /// Total scaling events by service and direction
    pub scale_events_total: CounterVec,
    /// Total scale up events
    pub scale_up_total: Counter,
    /// Total scale down events
    pub scale_down_total: Counter,

    // Request metrics
    /// Total HTTP requests by method, path, and status
    pub requests_total: CounterVec,
    /// HTTP request duration in seconds
    pub request_duration_seconds: HistogramVec,

    // Raft metrics
    /// Whether this node is the Raft leader (1) or not (0)
    pub raft_is_leader: Gauge,
    /// Current Raft term
    pub raft_term: Gauge,
    /// Current Raft commit index
    pub raft_commit_index: Gauge,

    // System metrics
    /// Time since the service started in seconds
    pub uptime_seconds: Gauge,
}

impl ZLayerMetrics {
    /// Create a new metrics collection
    pub fn new() -> Result<Self> {
        let registry = Registry::new();

        // Service metrics
        let services_total = Gauge::new("zlayer_services_total", "Total number of registered services")
            .map_err(|e| ObservabilityError::MetricsInit(e.to_string()))?;

        let service_replicas = GaugeVec::new(
            Opts::new(
                "zlayer_service_replicas",
                "Current replica count per service",
            ),
            &["service"],
        )
        .map_err(|e| ObservabilityError::MetricsInit(e.to_string()))?;

        let service_health = GaugeVec::new(
            Opts::new(
                "zlayer_service_health",
                "Service health status (0=unknown, 1=healthy, 2=degraded, 3=unhealthy)",
            ),
            &["service"],
        )
        .map_err(|e| ObservabilityError::MetricsInit(e.to_string()))?;

        // Scaling metrics
        let scale_events_total = CounterVec::new(
            Opts::new("zlayer_scale_events_total", "Total scaling events"),
            &["service", "direction"],
        )
        .map_err(|e| ObservabilityError::MetricsInit(e.to_string()))?;

        let scale_up_total =
            Counter::new("zlayer_scale_up_total", "Total scale up events")
                .map_err(|e| ObservabilityError::MetricsInit(e.to_string()))?;

        let scale_down_total =
            Counter::new("zlayer_scale_down_total", "Total scale down events")
                .map_err(|e| ObservabilityError::MetricsInit(e.to_string()))?;

        // Request metrics
        let requests_total = CounterVec::new(
            Opts::new("zlayer_requests_total", "Total HTTP requests"),
            &["method", "path", "status"],
        )
        .map_err(|e| ObservabilityError::MetricsInit(e.to_string()))?;

        let request_duration_seconds = HistogramVec::new(
            prometheus::HistogramOpts::new(
                "zlayer_request_duration_seconds",
                "HTTP request duration in seconds",
            )
            .buckets(vec![
                0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
            ]),
            &["method", "path"],
        )
        .map_err(|e| ObservabilityError::MetricsInit(e.to_string()))?;

        // Raft metrics
        let raft_is_leader = Gauge::new(
            "zlayer_raft_is_leader",
            "Whether this node is the Raft leader (1) or not (0)",
        )
        .map_err(|e| ObservabilityError::MetricsInit(e.to_string()))?;

        let raft_term = Gauge::new("zlayer_raft_term", "Current Raft term")
            .map_err(|e| ObservabilityError::MetricsInit(e.to_string()))?;

        let raft_commit_index =
            Gauge::new("zlayer_raft_commit_index", "Current Raft commit index")
                .map_err(|e| ObservabilityError::MetricsInit(e.to_string()))?;

        // System metrics
        let uptime_seconds = Gauge::new(
            "zlayer_uptime_seconds",
            "Time since the service started in seconds",
        )
        .map_err(|e| ObservabilityError::MetricsInit(e.to_string()))?;

        // Register all metrics
        registry
            .register(Box::new(services_total.clone()))
            .ok();
        registry
            .register(Box::new(service_replicas.clone()))
            .ok();
        registry
            .register(Box::new(service_health.clone()))
            .ok();
        registry
            .register(Box::new(scale_events_total.clone()))
            .ok();
        registry
            .register(Box::new(scale_up_total.clone()))
            .ok();
        registry
            .register(Box::new(scale_down_total.clone()))
            .ok();
        registry
            .register(Box::new(requests_total.clone()))
            .ok();
        registry
            .register(Box::new(request_duration_seconds.clone()))
            .ok();
        registry
            .register(Box::new(raft_is_leader.clone()))
            .ok();
        registry.register(Box::new(raft_term.clone())).ok();
        registry
            .register(Box::new(raft_commit_index.clone()))
            .ok();
        registry
            .register(Box::new(uptime_seconds.clone()))
            .ok();

        Ok(Self {
            registry,
            services_total,
            service_replicas,
            service_health,
            scale_events_total,
            scale_up_total,
            scale_down_total,
            requests_total,
            request_duration_seconds,
            raft_is_leader,
            raft_term,
            raft_commit_index,
            uptime_seconds,
        })
    }

    /// Get the Prometheus registry
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Encode metrics in Prometheus text format
    pub fn encode(&self) -> Result<String> {
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();
        let mut buffer = Vec::new();
        encoder
            .encode(&metric_families, &mut buffer)
            .map_err(|e| ObservabilityError::MetricsInit(e.to_string()))?;
        String::from_utf8(buffer).map_err(|e| ObservabilityError::MetricsInit(e.to_string()))
    }

    /// Record a scale event
    pub fn record_scale_event(&self, service: &str, direction: &str) {
        self.scale_events_total
            .with_label_values(&[service, direction])
            .inc();

        match direction {
            "up" => self.scale_up_total.inc(),
            "down" => self.scale_down_total.inc(),
            _ => {}
        }
    }

    /// Update service replica count
    pub fn set_replicas(&self, service: &str, count: u32) {
        self.service_replicas
            .with_label_values(&[service])
            .set(count as f64);
    }

    /// Update service health status
    pub fn set_health(&self, service: &str, health: HealthStatus) {
        self.service_health
            .with_label_values(&[service])
            .set(health as i32 as f64);
    }

    /// Record an HTTP request
    pub fn record_request(&self, method: &str, path: &str, status: u16, duration_secs: f64) {
        let status_str = status.to_string();
        self.requests_total
            .with_label_values(&[method, path, &status_str])
            .inc();
        self.request_duration_seconds
            .with_label_values(&[method, path])
            .observe(duration_secs);
    }

    /// Update Raft leader status
    pub fn set_raft_leader(&self, is_leader: bool) {
        self.raft_is_leader.set(if is_leader { 1.0 } else { 0.0 });
    }

    /// Update Raft term
    pub fn set_raft_term(&self, term: u64) {
        self.raft_term.set(term as f64);
    }

    /// Update Raft commit index
    pub fn set_raft_commit_index(&self, index: u64) {
        self.raft_commit_index.set(index as f64);
    }

    /// Update uptime
    pub fn set_uptime(&self, seconds: f64) {
        self.uptime_seconds.set(seconds);
    }
}

impl Default for ZLayerMetrics {
    fn default() -> Self {
        Self::new().expect("Failed to create default metrics")
    }
}

/// Health status values for metrics
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum HealthStatus {
    /// Unknown health status
    Unknown = 0,
    /// Service is healthy
    Healthy = 1,
    /// Service is degraded
    Degraded = 2,
    /// Service is unhealthy
    Unhealthy = 3,
}

/// Global metrics instance
static METRICS: std::sync::OnceLock<ZLayerMetrics> = std::sync::OnceLock::new();

/// Initialize global metrics
pub fn init_metrics(config: &MetricsConfig) -> Result<&'static ZLayerMetrics> {
    if !config.enabled {
        info!("Metrics disabled by configuration");
    }

    METRICS.get_or_init(|| ZLayerMetrics::new().expect("Failed to initialize metrics"));

    Ok(METRICS.get().unwrap())
}

/// Get the global metrics instance
pub fn metrics() -> Option<&'static ZLayerMetrics> {
    METRICS.get()
}

/// Axum handler for Prometheus metrics endpoint
#[cfg(feature = "axum")]
pub async fn metrics_handler() -> impl axum::response::IntoResponse {
    use axum::http::StatusCode;

    match metrics() {
        Some(m) => match m.encode() {
            Ok(body) => (StatusCode::OK, body),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Error: {}", e),
            ),
        },
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            "Metrics not initialized".to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_creation() {
        let metrics = ZLayerMetrics::new().unwrap();

        // Test basic operations
        metrics.services_total.set(5.0);
        metrics.set_replicas("api", 3);
        metrics.record_scale_event("api", "up");

        // Should be able to encode
        let encoded = metrics.encode().unwrap();
        assert!(encoded.contains("zlayer_services_total"));
        assert!(encoded.contains("zlayer_service_replicas"));
        assert!(encoded.contains("zlayer_scale_events_total"));
    }

    #[test]
    fn test_request_recording() {
        let metrics = ZLayerMetrics::new().unwrap();

        metrics.record_request("GET", "/api/health", 200, 0.015);
        metrics.record_request("POST", "/api/deploy", 201, 1.234);

        let encoded = metrics.encode().unwrap();
        assert!(encoded.contains("zlayer_requests_total"));
        assert!(encoded.contains("zlayer_request_duration_seconds"));
    }

    #[test]
    fn test_raft_metrics() {
        let metrics = ZLayerMetrics::new().unwrap();

        metrics.set_raft_leader(true);
        metrics.set_raft_term(42);
        metrics.set_raft_commit_index(100);

        let encoded = metrics.encode().unwrap();
        assert!(encoded.contains("zlayer_raft_is_leader 1"));
        assert!(encoded.contains("zlayer_raft_term 42"));
        assert!(encoded.contains("zlayer_raft_commit_index 100"));
    }

    #[test]
    fn test_health_status() {
        let metrics = ZLayerMetrics::new().unwrap();

        metrics.set_health("api", HealthStatus::Healthy);
        metrics.set_health("db", HealthStatus::Degraded);

        let encoded = metrics.encode().unwrap();
        assert!(encoded.contains("zlayer_service_health"));
    }

    #[test]
    fn test_scale_events() {
        let metrics = ZLayerMetrics::new().unwrap();

        metrics.record_scale_event("api", "up");
        metrics.record_scale_event("api", "up");
        metrics.record_scale_event("api", "down");

        let encoded = metrics.encode().unwrap();
        assert!(encoded.contains("zlayer_scale_up_total 2"));
        assert!(encoded.contains("zlayer_scale_down_total 1"));
    }
}
