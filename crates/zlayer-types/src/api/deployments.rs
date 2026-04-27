//! Deployment DTOs.

use serde::{Deserialize, Serialize};

/// Deployment summary
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DeploymentSummary {
    /// Deployment name
    pub name: String,
    /// Deployment status
    pub status: String,
    /// Number of services
    pub service_count: usize,
    /// Created timestamp
    pub created_at: String,
}

/// Per-service health info included in deployment details
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ServiceHealthInfo {
    /// Service name
    pub name: String,
    /// Running replica count
    pub replicas_running: u32,
    /// Desired replica count
    pub replicas_desired: u32,
    /// Health status ("healthy", "unhealthy", "unknown")
    pub health: String,
    /// Endpoint URLs for this service
    pub endpoints: Vec<String>,
}

/// Deployment details (enhanced with per-service health and endpoints)
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DeploymentDetails {
    /// Deployment name
    pub name: String,
    /// Deployment status
    pub status: String,
    /// Service names (for backwards compatibility)
    pub services: Vec<String>,
    /// Per-service health and endpoint info
    pub service_health: Vec<ServiceHealthInfo>,
    /// Created timestamp
    pub created_at: String,
    /// Updated timestamp
    pub updated_at: String,
}

/// Create deployment request
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateDeploymentRequest {
    /// Deployment specification (YAML content)
    pub spec: String,
}

/// Deployment progress event sent over SSE during orchestration.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeploymentProgressEvent {
    /// Deployment orchestration has started
    Started {
        /// Deployment name
        deployment: String,
        /// List of services being deployed
        services: Vec<String>,
    },
    /// A service was successfully registered with the service manager
    ServiceRegistered {
        /// Service name
        service: String,
    },
    /// A service failed to register
    ServiceRegistrationFailed {
        /// Service name
        service: String,
        /// Error message
        error: String,
    },
    /// Overlay network created for a service
    OverlayCreated {
        /// Service name
        service: String,
        /// Network interface name
        interface: String,
    },
    /// Overlay creation failed (non-fatal)
    OverlayFailed {
        /// Service name
        service: String,
        /// Error message
        error: String,
    },
    /// Proxy routes configured for a service
    ProxyConfigured {
        /// Service name
        service: String,
    },
    /// Proxy configuration failed (non-fatal)
    ProxyFailed {
        /// Service name
        service: String,
        /// Error message
        error: String,
    },
    /// Service scaling has started
    ServiceScaling {
        /// Service name
        service: String,
        /// Target replica count
        target: u32,
    },
    /// Service successfully scaled
    ServiceScaled {
        /// Service name
        service: String,
        /// Number of replicas running
        replicas: u32,
    },
    /// Service scaling failed
    ServiceScaleFailed {
        /// Service name
        service: String,
        /// Error message
        error: String,
    },
    /// Waiting for stabilization
    Stabilizing,
    /// Deployment is ready and running
    Ready,
    /// Deployment failed
    Failed {
        /// Error message describing the failure
        message: String,
    },
}

/// Wrapper for serializing deployment progress events as SSE.
///
/// Converts each [`DeploymentProgressEvent`] variant into an `event_type` string
/// and a JSON `data` payload, following the same pattern as `BuildEventWrapper`.
#[derive(Debug, Clone, Serialize)]
pub struct DeploymentEventWrapper {
    /// SSE event type (used as the `event:` field)
    #[serde(rename = "type")]
    pub event_type: String,
    /// JSON data payload
    pub data: serde_json::Value,
}

impl From<DeploymentProgressEvent> for DeploymentEventWrapper {
    fn from(event: DeploymentProgressEvent) -> Self {
        match event {
            DeploymentProgressEvent::Started {
                deployment,
                services,
            } => DeploymentEventWrapper {
                event_type: "started".to_string(),
                data: serde_json::json!({
                    "deployment": deployment,
                    "services": services,
                }),
            },
            DeploymentProgressEvent::ServiceRegistered { service } => DeploymentEventWrapper {
                event_type: "service_registered".to_string(),
                data: serde_json::json!({ "service": service }),
            },
            DeploymentProgressEvent::ServiceRegistrationFailed { service, error } => {
                DeploymentEventWrapper {
                    event_type: "service_registration_failed".to_string(),
                    data: serde_json::json!({ "service": service, "error": error }),
                }
            }
            DeploymentProgressEvent::OverlayCreated { service, interface } => {
                DeploymentEventWrapper {
                    event_type: "overlay_created".to_string(),
                    data: serde_json::json!({ "service": service, "interface": interface }),
                }
            }
            DeploymentProgressEvent::OverlayFailed { service, error } => DeploymentEventWrapper {
                event_type: "overlay_failed".to_string(),
                data: serde_json::json!({ "service": service, "error": error }),
            },
            DeploymentProgressEvent::ProxyConfigured { service } => DeploymentEventWrapper {
                event_type: "proxy_configured".to_string(),
                data: serde_json::json!({ "service": service }),
            },
            DeploymentProgressEvent::ProxyFailed { service, error } => DeploymentEventWrapper {
                event_type: "proxy_failed".to_string(),
                data: serde_json::json!({ "service": service, "error": error }),
            },
            DeploymentProgressEvent::ServiceScaling { service, target } => DeploymentEventWrapper {
                event_type: "service_scaling".to_string(),
                data: serde_json::json!({ "service": service, "target": target }),
            },
            DeploymentProgressEvent::ServiceScaled { service, replicas } => {
                DeploymentEventWrapper {
                    event_type: "service_scaled".to_string(),
                    data: serde_json::json!({ "service": service, "replicas": replicas }),
                }
            }
            DeploymentProgressEvent::ServiceScaleFailed { service, error } => {
                DeploymentEventWrapper {
                    event_type: "service_scale_failed".to_string(),
                    data: serde_json::json!({ "service": service, "error": error }),
                }
            }
            DeploymentProgressEvent::Stabilizing => DeploymentEventWrapper {
                event_type: "stabilizing".to_string(),
                data: serde_json::json!({}),
            },
            DeploymentProgressEvent::Ready => DeploymentEventWrapper {
                event_type: "ready".to_string(),
                data: serde_json::json!({}),
            },
            DeploymentProgressEvent::Failed { message } => DeploymentEventWrapper {
                event_type: "failed".to_string(),
                data: serde_json::json!({ "message": message }),
            },
        }
    }
}
