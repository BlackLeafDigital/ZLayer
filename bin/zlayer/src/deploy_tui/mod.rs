//! Deploy TUI event system for deployment progress visualization
//!
//! This module provides the event types and plain logger for displaying
//! deployment progress. It follows the same event-driven architecture as
//! the build TUI in `zlayer-builder`.
//!
//! # Architecture
//!
//! ```text
//! +-------------------+     mpsc::Sender<DeployEvent>     +--------------------+
//! | deploy_services() | --------------------------------> | PlainDeployLogger  |
//! +-------------------+                                   +--------------------+
//! ```
//!
//! The deploy orchestration sends events through a channel, and the logger
//! (or future TUI) consumes them to render progress output.

pub mod app;
pub mod deploy_view;
pub mod logger;
pub mod state;
pub mod widgets;

pub use logger::PlainDeployLogger;

/// Deployment event for TUI/logger updates
///
/// These events are sent from the deploy orchestration to update the display.
/// The logger processes them synchronously and prints structured output.
#[derive(Debug, Clone)]
pub enum DeployEvent {
    /// Deployment plan is ready to display
    PlanReady {
        /// Name of the deployment
        deployment_name: String,
        /// Spec version string
        version: String,
        /// Planned services to deploy
        services: Vec<ServicePlan>,
    },

    /// An infrastructure phase has started
    InfraPhaseStarted {
        /// Which infrastructure component is starting
        phase: InfraPhase,
    },

    /// An infrastructure phase has completed
    InfraPhaseComplete {
        /// Which infrastructure component completed
        phase: InfraPhase,
        /// Whether the phase succeeded
        success: bool,
        /// Optional message (e.g. warning or address info)
        message: Option<String>,
    },

    /// A service deployment has started
    ServiceDeployStarted {
        /// Service name
        name: String,
    },

    /// A service was successfully registered with the service manager
    ServiceRegistered {
        /// Service name
        name: String,
    },

    /// A service is being scaled to target replicas
    ServiceScaling {
        /// Service name
        name: String,
        /// Target replica count
        target_replicas: u32,
    },

    /// Periodic update during stabilization wait
    ServiceReplicaUpdate {
        /// Service name
        name: String,
        /// Current running replica count
        current: u32,
        /// Target replica count
        target: u32,
    },

    /// A service deployment completed successfully
    ServiceDeployComplete {
        /// Service name
        name: String,
        /// Final replica count
        replicas: u32,
    },

    /// A service deployment failed
    ServiceDeployFailed {
        /// Service name
        name: String,
        /// Error description
        error: String,
    },

    /// All services are running (deployment summary)
    DeploymentRunning {
        /// List of (service_name, replica_count) pairs
        services: Vec<(String, u32)>,
    },

    /// Periodic status tick during foreground wait (silenced by plain logger)
    StatusTick {
        /// Current status of each service
        services: Vec<ServiceStatus>,
    },

    /// Shutdown sequence has started
    ShutdownStarted,

    /// A specific service is being stopped
    ServiceStopping {
        /// Service name
        name: String,
    },

    /// A specific service has stopped
    ServiceStopped {
        /// Service name
        name: String,
    },

    /// Shutdown sequence is complete
    ShutdownComplete,

    /// General log message
    Log {
        /// Severity level
        level: LogLevel,
        /// Log message text
        message: String,
    },
}

/// Plan for a single service within a deployment
#[derive(Debug, Clone)]
pub struct ServicePlan {
    /// Service name
    pub name: String,
    /// Container image reference
    pub image: String,
    /// Scaling mode description (e.g. "fixed(3)", "adaptive(1-10)", "manual")
    pub scale_mode: String,
    /// Endpoint summaries (e.g. "http:8080 (public)")
    pub endpoints: Vec<String>,
}

/// Infrastructure phase identifiers
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfraPhase {
    /// Container runtime initialization
    Runtime,
    /// Encrypted overlay network
    Overlay,
    /// DNS server
    Dns,
    /// L4/L7 proxy manager
    Proxy,
    /// Container supervisor loop
    Supervisor,
    /// REST API server
    Api,
}

impl std::fmt::Display for InfraPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InfraPhase::Runtime => write!(f, "Container Runtime"),
            InfraPhase::Overlay => write!(f, "Overlay Network"),
            InfraPhase::Dns => write!(f, "DNS Server"),
            InfraPhase::Proxy => write!(f, "Proxy Manager"),
            InfraPhase::Supervisor => write!(f, "Container Supervisor"),
            InfraPhase::Api => write!(f, "API Server"),
        }
    }
}

/// Runtime status of a deployed service
#[derive(Debug, Clone)]
pub struct ServiceStatus {
    /// Service name
    pub name: String,
    /// Number of replicas currently running
    pub replicas_running: u32,
    /// Target replica count
    pub replicas_target: u32,
    /// Overall health of the service
    pub health: ServiceHealth,
}

/// Health state of a service
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceHealth {
    /// All replicas healthy
    Healthy,
    /// Some replicas unhealthy or missing
    Degraded,
    /// All replicas unhealthy or none running
    Unhealthy,
    /// Health status not yet determined
    Unknown,
}

/// Log severity level for deploy events (re-exported from zlayer-tui)
pub use zlayer_tui::widgets::scrollable_pane::LogLevel;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infra_phase_display() {
        assert_eq!(InfraPhase::Runtime.to_string(), "Container Runtime");
        assert_eq!(InfraPhase::Overlay.to_string(), "Overlay Network");
        assert_eq!(InfraPhase::Dns.to_string(), "DNS Server");
        assert_eq!(InfraPhase::Proxy.to_string(), "Proxy Manager");
        assert_eq!(InfraPhase::Supervisor.to_string(), "Container Supervisor");
        assert_eq!(InfraPhase::Api.to_string(), "API Server");
    }

    #[test]
    fn test_deploy_event_debug() {
        let event = DeployEvent::PlanReady {
            deployment_name: "test".to_string(),
            version: "v1".to_string(),
            services: vec![],
        };
        // Ensure Debug trait works
        let _ = format!("{:?}", event);
    }

    #[test]
    fn test_deploy_event_clone() {
        let event = DeployEvent::ServiceDeployStarted {
            name: "web".to_string(),
        };
        let cloned = event.clone();
        match cloned {
            DeployEvent::ServiceDeployStarted { name } => assert_eq!(name, "web"),
            _ => panic!("Clone produced wrong variant"),
        }
    }

    #[test]
    fn test_service_plan_fields() {
        let plan = ServicePlan {
            name: "api".to_string(),
            image: "myapp:latest".to_string(),
            scale_mode: "fixed(3)".to_string(),
            endpoints: vec!["http:8080 (public)".to_string()],
        };
        assert_eq!(plan.name, "api");
        assert_eq!(plan.image, "myapp:latest");
        assert_eq!(plan.scale_mode, "fixed(3)");
        assert_eq!(plan.endpoints.len(), 1);
    }

    #[test]
    fn test_service_health_equality() {
        assert_eq!(ServiceHealth::Healthy, ServiceHealth::Healthy);
        assert_ne!(ServiceHealth::Healthy, ServiceHealth::Degraded);
        assert_ne!(ServiceHealth::Unhealthy, ServiceHealth::Unknown);
    }

    #[test]
    fn test_log_level_equality() {
        assert_eq!(LogLevel::Info, LogLevel::Info);
        assert_ne!(LogLevel::Info, LogLevel::Warn);
        assert_ne!(LogLevel::Warn, LogLevel::Error);
    }
}
