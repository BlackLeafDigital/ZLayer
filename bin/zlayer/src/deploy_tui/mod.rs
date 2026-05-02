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

    /// Service registration is about to begin (pre-register sub-phase)
    ServiceRegistrationStarted {
        /// Service name
        name: String,
    },

    /// A service was successfully registered with the service manager
    ServiceRegistered {
        /// Service name
        name: String,
    },

    /// Overlay network setup is about to begin (pre-overlay sub-phase)
    ServiceOverlaySetupStarted {
        /// Service name
        name: String,
    },

    /// Proxy setup is about to begin (pre-proxy sub-phase)
    ServiceProxySetupStarted {
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

    /// Image pull is about to begin
    ImagePullStarted {
        /// Image reference (e.g. `nginx:latest`)
        image: String,
    },

    /// Image pull completed successfully
    ImagePullComplete {
        /// Image reference (e.g. `nginx:latest`)
        image: String,
        /// Resolved image digest (e.g. `sha256:abc...`)
        digest: String,
    },

    /// Re-deploy detected no digest drift; service is up-to-date and skipped
    ServiceUpToDate {
        /// Service name
        name: String,
        /// Image digest currently running
        digest: String,
    },

    /// Re-deploy detected drift; service is being recreated with a new image
    ServiceRecreating {
        /// Service name
        name: String,
        /// Previous image digest
        old_digest: String,
        /// New image digest
        new_digest: String,
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
        /// List of (`service_name`, `replica_count`) pairs
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
    /// Endpoint summaries (e.g. "<http:8080> (public)")
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

/// Convert a daemon SSE event (decoded `event_type` + JSON `data` payload)
/// into one or more `DeployEvent`s for the TUI/logger.
///
/// This is the canonical SSE -> `DeployEvent` translator for the deploy TUI.
/// It returns `Vec<DeployEvent>` to allow a single SSE event to expand into
/// multiple TUI events (none today, but preserves flexibility) and an empty
/// `Vec` for events the TUI does not render. Unknown event types fall through
/// to `Log { level: Info, message: ... }` so the TUI does not silently drop
/// daemon updates introduced after the CLI was built.
///
/// The match is **case-insensitive** on `event_type` so a daemon that adopts
/// a different casing convention does not silently break the TUI.
#[must_use]
pub fn sse_to_deploy_events(event_type: &str, data: &serde_json::Value) -> Vec<DeployEvent> {
    let kind = event_type.to_ascii_lowercase();
    let svc = || {
        data.get("service")
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    let err = || {
        data.get("error")
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };

    match kind.as_str() {
        "service_registration_started" => svc()
            .map(|name| vec![DeployEvent::ServiceRegistrationStarted { name }])
            .unwrap_or_default(),
        "overlay_setup_started" => svc()
            .map(|name| vec![DeployEvent::ServiceOverlaySetupStarted { name }])
            .unwrap_or_default(),
        "proxy_setup_started" => svc()
            .map(|name| vec![DeployEvent::ServiceProxySetupStarted { name }])
            .unwrap_or_default(),
        "stabilization_progress" => {
            let Some(name) = svc() else { return Vec::new() };
            let u32_field = |k: &str| {
                u32::try_from(data.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0))
                    .unwrap_or(u32::MAX)
            };
            vec![DeployEvent::ServiceReplicaUpdate {
                name,
                current: u32_field("replicas_running"),
                target: u32_field("target"),
            }]
        }
        "image_pull_started" => data
            .get("image")
            .and_then(|v| v.as_str())
            .map(|image| {
                vec![DeployEvent::ImagePullStarted {
                    image: image.to_string(),
                }]
            })
            .unwrap_or_default(),
        "image_pull_complete" => {
            let Some(image) = data.get("image").and_then(|v| v.as_str()) else {
                return Vec::new();
            };
            let digest = data
                .get("digest")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            vec![DeployEvent::ImagePullComplete {
                image: image.to_string(),
                digest,
            }]
        }
        "service_up_to_date" => {
            let Some(name) = svc() else { return Vec::new() };
            let digest = data
                .get("digest")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            vec![DeployEvent::ServiceUpToDate { name, digest }]
        }
        "service_recreating" => {
            let Some(name) = svc() else { return Vec::new() };
            let old_digest = data
                .get("old_digest")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let new_digest = data
                .get("new_digest")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            vec![DeployEvent::ServiceRecreating {
                name,
                old_digest,
                new_digest,
            }]
        }
        // Failure variants are mapped to ServiceDeployFailed so the TUI shows
        // a red row even if the upstream caller hasn't wired them yet.
        "service_registration_failed"
        | "overlay_failed"
        | "proxy_failed"
        | "service_scale_failed" => {
            let Some(name) = svc() else { return Vec::new() };
            let error = err().unwrap_or_else(|| format!("{event_type} (no error message)"));
            vec![DeployEvent::ServiceDeployFailed { name, error }]
        }
        // Events handled by the existing translator in `commands/deploy.rs`
        // are intentionally not duplicated here. An unknown event type
        // becomes a Log so the TUI surfaces it instead of dropping silently.
        _ => Vec::new(),
    }
}

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
        let _ = format!("{event:?}");
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

    #[test]
    fn test_sse_translator_registration_started() {
        let data = serde_json::json!({ "service": "web" });
        let events = sse_to_deploy_events("service_registration_started", &data);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            DeployEvent::ServiceRegistrationStarted { ref name } if name == "web"
        ));
    }

    #[test]
    fn test_sse_translator_overlay_setup_started() {
        let data = serde_json::json!({ "service": "api" });
        let events = sse_to_deploy_events("overlay_setup_started", &data);
        assert!(matches!(
            events[0],
            DeployEvent::ServiceOverlaySetupStarted { ref name } if name == "api"
        ));
    }

    #[test]
    fn test_sse_translator_proxy_setup_started() {
        let data = serde_json::json!({ "service": "edge" });
        let events = sse_to_deploy_events("proxy_setup_started", &data);
        assert!(matches!(
            events[0],
            DeployEvent::ServiceProxySetupStarted { ref name } if name == "edge"
        ));
    }

    #[test]
    fn test_sse_translator_stabilization_progress() {
        let data = serde_json::json!({
            "service": "web",
            "replicas_running": 2,
            "target": 3,
        });
        let events = sse_to_deploy_events("stabilization_progress", &data);
        assert_eq!(events.len(), 1);
        match &events[0] {
            DeployEvent::ServiceReplicaUpdate {
                name,
                current,
                target,
            } => {
                assert_eq!(name, "web");
                assert_eq!(*current, 2);
                assert_eq!(*target, 3);
            }
            other => panic!("expected ServiceReplicaUpdate, got {other:?}"),
        }
    }

    #[test]
    fn test_sse_translator_image_pull() {
        let started =
            sse_to_deploy_events("image_pull_started", &serde_json::json!({ "image": "n:1" }));
        assert!(matches!(
            started[0],
            DeployEvent::ImagePullStarted { ref image } if image == "n:1"
        ));

        let complete = sse_to_deploy_events(
            "image_pull_complete",
            &serde_json::json!({ "image": "n:1", "digest": "sha256:abc" }),
        );
        match &complete[0] {
            DeployEvent::ImagePullComplete { image, digest } => {
                assert_eq!(image, "n:1");
                assert_eq!(digest, "sha256:abc");
            }
            other => panic!("expected ImagePullComplete, got {other:?}"),
        }
    }

    #[test]
    fn test_sse_translator_up_to_date() {
        let events = sse_to_deploy_events(
            "service_up_to_date",
            &serde_json::json!({ "service": "api", "digest": "sha256:abc" }),
        );
        match &events[0] {
            DeployEvent::ServiceUpToDate { name, digest } => {
                assert_eq!(name, "api");
                assert_eq!(digest, "sha256:abc");
            }
            other => panic!("expected ServiceUpToDate, got {other:?}"),
        }
    }

    #[test]
    fn test_sse_translator_recreating() {
        let events = sse_to_deploy_events(
            "service_recreating",
            &serde_json::json!({
                "service": "api",
                "old_digest": "sha256:111",
                "new_digest": "sha256:222",
            }),
        );
        match &events[0] {
            DeployEvent::ServiceRecreating {
                name,
                old_digest,
                new_digest,
            } => {
                assert_eq!(name, "api");
                assert_eq!(old_digest, "sha256:111");
                assert_eq!(new_digest, "sha256:222");
            }
            other => panic!("expected ServiceRecreating, got {other:?}"),
        }
    }

    #[test]
    fn test_sse_translator_unknown_returns_empty() {
        let events = sse_to_deploy_events("totally_made_up", &serde_json::json!({}));
        assert!(events.is_empty());
    }

    #[test]
    fn test_sse_translator_case_insensitive() {
        let events = sse_to_deploy_events(
            "Service_Registration_Started",
            &serde_json::json!({ "service": "web" }),
        );
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn test_sse_translator_failure_variants() {
        let events = sse_to_deploy_events(
            "service_registration_failed",
            &serde_json::json!({ "service": "web", "error": "boom" }),
        );
        assert!(matches!(
            events[0],
            DeployEvent::ServiceDeployFailed { ref name, ref error }
                if name == "web" && error == "boom"
        ));
    }
}
