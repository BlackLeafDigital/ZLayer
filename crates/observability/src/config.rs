//! Configuration types for observability

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Log output format
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// Human-readable pretty format
    #[default]
    Pretty,
    /// JSON format for log aggregation
    Json,
    /// Compact format (single line)
    Compact,
}

/// Log level
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

impl From<LogLevel> for tracing::Level {
    fn from(level: LogLevel) -> Self {
        match level {
            LogLevel::Trace => tracing::Level::TRACE,
            LogLevel::Debug => tracing::Level::DEBUG,
            LogLevel::Info => tracing::Level::INFO,
            LogLevel::Warn => tracing::Level::WARN,
            LogLevel::Error => tracing::Level::ERROR,
        }
    }
}

impl From<LogLevel> for tracing_subscriber::filter::LevelFilter {
    fn from(level: LogLevel) -> Self {
        match level {
            LogLevel::Trace => tracing_subscriber::filter::LevelFilter::TRACE,
            LogLevel::Debug => tracing_subscriber::filter::LevelFilter::DEBUG,
            LogLevel::Info => tracing_subscriber::filter::LevelFilter::INFO,
            LogLevel::Warn => tracing_subscriber::filter::LevelFilter::WARN,
            LogLevel::Error => tracing_subscriber::filter::LevelFilter::ERROR,
        }
    }
}

/// Logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Log level
    #[serde(default)]
    pub level: LogLevel,

    /// Output format
    #[serde(default)]
    pub format: LogFormat,

    /// Log to file (optional)
    #[serde(default)]
    pub file: Option<FileLoggingConfig>,

    /// Include source code location in logs
    #[serde(default = "default_true")]
    pub include_location: bool,

    /// Include target (module path) in logs
    #[serde(default = "default_true")]
    pub include_target: bool,
}

fn default_true() -> bool {
    true
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: LogLevel::Info,
            format: LogFormat::Pretty,
            file: None,
            include_location: true,
            include_target: true,
        }
    }
}

/// File logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileLoggingConfig {
    /// Directory for log files
    pub directory: PathBuf,

    /// File name prefix
    #[serde(default = "default_prefix")]
    pub prefix: String,

    /// Rotation strategy
    #[serde(default)]
    pub rotation: RotationStrategy,
}

fn default_prefix() -> String {
    "zlayer".to_string()
}

/// Log file rotation strategy
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RotationStrategy {
    /// Rotate daily
    #[default]
    Daily,
    /// Rotate hourly
    Hourly,
    /// Never rotate (single file)
    Never,
}

/// Metrics configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsConfig {
    /// Enable Prometheus metrics
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Metrics endpoint path
    #[serde(default = "default_metrics_path")]
    pub path: String,

    /// Port for standalone metrics server (if not using API)
    #[serde(default)]
    pub port: Option<u16>,
}

fn default_metrics_path() -> String {
    "/metrics".to_string()
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path: default_metrics_path(),
            port: None,
        }
    }
}

/// Tracing (OpenTelemetry) configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracingConfig {
    /// Enable distributed tracing
    #[serde(default)]
    pub enabled: bool,

    /// OTLP endpoint (e.g., "http://localhost:4317")
    #[serde(default)]
    pub otlp_endpoint: Option<String>,

    /// Service name for traces
    #[serde(default = "default_service_name")]
    pub service_name: String,

    /// Sampling ratio (0.0 to 1.0)
    #[serde(default = "default_sampling_ratio")]
    pub sampling_ratio: f64,

    /// Deployment environment (production, staging, development)
    #[serde(default)]
    pub environment: Option<String>,

    /// Batch export configuration
    #[serde(default)]
    pub batch: BatchConfig,

    /// Use gRPC (true) or HTTP (false) for OTLP
    #[serde(default = "default_true")]
    pub use_grpc: bool,
}

fn default_service_name() -> String {
    "zlayer".to_string()
}

fn default_sampling_ratio() -> f64 {
    1.0
}

fn default_max_queue_size() -> usize {
    2048
}

fn default_scheduled_delay() -> u64 {
    5000
}

fn default_max_export_batch_size() -> usize {
    512
}

/// Batch export configuration for OpenTelemetry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchConfig {
    /// Maximum queue size before dropping spans (default: 2048)
    #[serde(default = "default_max_queue_size")]
    pub max_queue_size: usize,

    /// Scheduled delay for batch export in milliseconds (default: 5000)
    #[serde(default = "default_scheduled_delay")]
    pub scheduled_delay_ms: u64,

    /// Maximum export batch size (default: 512)
    #[serde(default = "default_max_export_batch_size")]
    pub max_export_batch_size: usize,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            max_queue_size: default_max_queue_size(),
            scheduled_delay_ms: default_scheduled_delay(),
            max_export_batch_size: default_max_export_batch_size(),
        }
    }
}

impl Default for TracingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            otlp_endpoint: None,
            service_name: default_service_name(),
            sampling_ratio: default_sampling_ratio(),
            environment: None,
            batch: BatchConfig::default(),
            use_grpc: true,
        }
    }
}

impl TracingConfig {
    /// Load from environment variables with fallback to defaults
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("OTEL_TRACES_ENABLED")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
            otlp_endpoint: std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok(),
            service_name: std::env::var("OTEL_SERVICE_NAME")
                .unwrap_or_else(|_| "zlayer".to_string()),
            sampling_ratio: std::env::var("OTEL_TRACES_SAMPLER_ARG")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1.0),
            environment: std::env::var("DEPLOYMENT_ENVIRONMENT").ok(),
            batch: BatchConfig::default(),
            use_grpc: std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL")
                .map(|v| v != "http/protobuf")
                .unwrap_or(true),
        }
    }
}

/// Complete observability configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ObservabilityConfig {
    /// Logging configuration
    #[serde(default)]
    pub logging: LoggingConfig,

    /// Metrics configuration
    #[serde(default)]
    pub metrics: MetricsConfig,

    /// Tracing configuration
    #[serde(default)]
    pub tracing: TracingConfig,
}
