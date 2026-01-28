//! ZLayer Observability - Logging, Tracing, and Metrics
//!
//! Provides unified observability infrastructure:
//! - Structured logging with JSON/pretty formats
//! - OpenTelemetry distributed tracing
//! - Prometheus metrics exposition
//!
//! # Quick Start
//!
//! ```no_run
//! use zlayer_observability::{init_observability, ObservabilityConfig};
//!
//! #[tokio::main]
//! async fn main() {
//!     let config = ObservabilityConfig::default();
//!     let _guards = init_observability(&config).expect("Failed to init observability");
//!
//!     tracing::info!("Application started");
//! }
//! ```

pub mod config;
pub mod container_spans;
pub mod error;
pub mod logging;
pub mod metrics;
pub mod propagation;
pub mod tracing_otel;

pub use config::*;
pub use container_spans::*;
pub use error::{ObservabilityError, Result};
pub use logging::{init_logging, LogGuard};
pub use metrics::{init_metrics, metrics, HealthStatus, ZLayerMetrics};
pub use tracing_otel::{create_otel_layer, init_tracing, TracingGuard};

/// Combined guards for all observability components
pub struct ObservabilityGuards {
    /// Guard for the logging system (keeps async file writer running)
    pub log_guard: LogGuard,
    /// Guard for the tracing system (flushes traces on drop)
    pub tracing_guard: TracingGuard,
}

/// Initialize all observability components
///
/// This is the recommended way to set up observability. It initializes:
/// - Logging (always)
/// - Metrics (always)
/// - Tracing (if enabled in config)
///
/// Returns guards that must be held for the lifetime of the application.
///
/// # Example
///
/// ```no_run
/// use zlayer_observability::{init_observability, ObservabilityConfig};
///
/// #[tokio::main]
/// async fn main() {
///     let config = ObservabilityConfig::default();
///     let _guards = init_observability(&config).expect("Failed to init observability");
///
///     tracing::info!("Application started");
///     // guards are dropped when main exits, flushing logs and traces
/// }
/// ```
pub fn init_observability(config: &ObservabilityConfig) -> Result<ObservabilityGuards> {
    // Initialize logging first so we can log from other init functions
    let log_guard = init_logging(&config.logging)?;

    // Initialize metrics
    let _ = init_metrics(&config.metrics)?;

    // Initialize tracing (may be disabled)
    let tracing_guard = init_tracing(&config.tracing)?;

    tracing::info!("Observability initialized");

    Ok(ObservabilityGuards {
        log_guard,
        tracing_guard,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ObservabilityConfig::default();
        assert!(!config.tracing.enabled);
        assert!(config.metrics.enabled);
    }
}
