//! Shared backend state for server functions
//!
//! This module provides a centralized state container for server-side resources
//! that need to be shared across multiple server functions. It is only compiled
//! when the `ssr` feature is enabled.
//!
//! # Usage
//!
//! The `WebState` struct is typically created once at application startup and
//! stored in Axum's state layer or Leptos context for access in server functions.
//!
//! ```ignore
//! use zlayer_web::server::state::WebState;
//! use std::sync::Arc;
//! use tokio::sync::RwLock;
//!
//! // Create state with optional components
//! let mut state = WebState::new();
//!
//! // Add service manager
//! let service_manager = Arc::new(RwLock::new(ServiceManager::new(runtime)));
//! state.set_service_manager(service_manager);
//!
//! // Use with Axum
//! let app = Router::new()
//!     .with_state(state);
//! ```

use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

use zlayer_agent::ServiceManager;
use zlayer_scheduler::metrics::MetricsCollector;

/// Shared backend state for server functions
///
/// This struct holds optional references to core `ZLayer` components that may be
/// needed by server functions. All fields are optional to support different
/// deployment configurations (e.g., web-only mode without agent integration).
///
/// # Thread Safety
///
/// - `ServiceManager` is wrapped in `Arc<RwLock<>>` because it requires mutable
///   access for operations like scaling services and managing deployments.
/// - `MetricsCollector` is wrapped in `Arc` only because it uses internal
///   synchronization (`RwLock` for cache) and only needs shared references.
/// - `start_time` is immutable after construction.
#[derive(Clone)]
pub struct WebState {
    /// Service manager for container lifecycle operations
    ///
    /// Wrapped in `Arc<RwLock<>>` to allow mutable operations like:
    /// - Scaling services up/down
    /// - Deploying new services
    /// - Removing services
    service_manager: Option<Arc<RwLock<ServiceManager>>>,

    /// Metrics collector for autoscaling decisions
    ///
    /// Wrapped in `Arc` since `MetricsCollector` uses internal synchronization
    /// and only requires shared references for collecting metrics.
    metrics_collector: Option<Arc<MetricsCollector>>,

    /// Server start time for uptime calculation
    ///
    /// Set automatically when `WebState` is created. Use `uptime()` to get
    /// the duration since the server started.
    start_time: Instant,
}

impl WebState {
    /// Create a new `WebState` with default values
    ///
    /// The server start time is automatically set to the current instant.
    /// All optional components are `None` and must be set using the builder
    /// methods or setters.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let state = WebState::new()
    ///     .with_service_manager(service_manager)
    ///     .with_metrics_collector(metrics_collector);
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            service_manager: None,
            metrics_collector: None,
            start_time: Instant::now(),
        }
    }

    /// Create a new `WebState` with all components configured
    ///
    /// This is a convenience constructor for when all components are available
    /// at initialization time.
    #[must_use]
    pub fn with_all(
        service_manager: Arc<RwLock<ServiceManager>>,
        metrics_collector: Arc<MetricsCollector>,
    ) -> Self {
        Self {
            service_manager: Some(service_manager),
            metrics_collector: Some(metrics_collector),
            start_time: Instant::now(),
        }
    }

    // =========================================================================
    // Builder methods
    // =========================================================================

    /// Set the service manager (builder pattern)
    #[must_use]
    pub fn with_service_manager(mut self, manager: Arc<RwLock<ServiceManager>>) -> Self {
        self.service_manager = Some(manager);
        self
    }

    /// Set the metrics collector (builder pattern)
    #[must_use]
    pub fn with_metrics_collector(mut self, collector: Arc<MetricsCollector>) -> Self {
        self.metrics_collector = Some(collector);
        self
    }

    // =========================================================================
    // Setters (for mutation after construction)
    // =========================================================================

    /// Set the service manager
    pub fn set_service_manager(&mut self, manager: Arc<RwLock<ServiceManager>>) {
        self.service_manager = Some(manager);
    }

    /// Set the metrics collector
    pub fn set_metrics_collector(&mut self, collector: Arc<MetricsCollector>) {
        self.metrics_collector = Some(collector);
    }

    // =========================================================================
    // Getters
    // =========================================================================

    /// Get a reference to the service manager, if configured
    #[must_use]
    pub fn service_manager(&self) -> Option<&Arc<RwLock<ServiceManager>>> {
        self.service_manager.as_ref()
    }

    /// Get a reference to the metrics collector, if configured
    #[must_use]
    pub fn metrics_collector(&self) -> Option<&Arc<MetricsCollector>> {
        self.metrics_collector.as_ref()
    }

    /// Get the server start time
    #[must_use]
    pub fn start_time(&self) -> Instant {
        self.start_time
    }

    /// Calculate the server uptime
    ///
    /// Returns the duration since the server was started.
    #[must_use]
    pub fn uptime(&self) -> std::time::Duration {
        self.start_time.elapsed()
    }

    // =========================================================================
    // Convenience checks
    // =========================================================================

    /// Check if the service manager is configured
    #[must_use]
    pub fn has_service_manager(&self) -> bool {
        self.service_manager.is_some()
    }

    /// Check if the metrics collector is configured
    #[must_use]
    pub fn has_metrics_collector(&self) -> bool {
        self.metrics_collector.is_some()
    }
}

impl Default for WebState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_webstate_default() {
        let state = WebState::default();
        assert!(state.service_manager.is_none());
        assert!(state.metrics_collector.is_none());
        assert!(!state.has_service_manager());
        assert!(!state.has_metrics_collector());
    }

    #[test]
    fn test_webstate_uptime() {
        let state = WebState::new();
        // Sleep a tiny bit to ensure uptime is non-zero
        std::thread::sleep(Duration::from_millis(10));
        let uptime = state.uptime();
        assert!(uptime >= Duration::from_millis(10));
    }

    #[test]
    fn test_webstate_with_metrics_collector() {
        let collector = Arc::new(MetricsCollector::new());
        let state = WebState::new().with_metrics_collector(collector);
        assert!(state.has_metrics_collector());
        assert!(state.metrics_collector().is_some());
    }

    #[test]
    fn test_webstate_set_metrics_collector() {
        let mut state = WebState::new();
        assert!(!state.has_metrics_collector());

        let collector = Arc::new(MetricsCollector::new());
        state.set_metrics_collector(collector);
        assert!(state.has_metrics_collector());
    }

    #[test]
    fn test_webstate_clone() {
        let collector = Arc::new(MetricsCollector::new());
        let state = WebState::new().with_metrics_collector(collector.clone());

        let cloned = state.clone();
        assert!(cloned.has_metrics_collector());

        // Verify both point to the same Arc
        assert!(Arc::ptr_eq(
            state.metrics_collector().unwrap(),
            cloned.metrics_collector().unwrap()
        ));
    }
}
