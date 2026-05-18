//! Service endpoint DTOs.

use serde::{Deserialize, Serialize};

use utoipa::IntoParams;

/// Service summary
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ServiceSummary {
    /// Service name
    pub name: String,
    /// Deployment name
    pub deployment: String,
    /// Service status
    pub status: String,
    /// Current replica count
    pub replicas: u32,
    /// Desired replica count
    pub desired_replicas: u32,
    /// Service endpoints
    pub endpoints: Vec<ServiceEndpoint>,
}

/// Service details
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ServiceDetails {
    /// Service name
    pub name: String,
    /// Deployment name
    pub deployment: String,
    /// Service status
    pub status: String,
    /// Current replica count
    pub replicas: u32,
    /// Desired replica count
    pub desired_replicas: u32,
    /// Service endpoints
    pub endpoints: Vec<ServiceEndpoint>,
    /// Service metrics
    pub metrics: ServiceMetrics,
}

/// Service endpoint
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ServiceEndpoint {
    /// Endpoint name
    pub name: String,
    /// Protocol
    pub protocol: String,
    /// Port
    pub port: u16,
    /// URL (if public)
    pub url: Option<String>,
}

/// Service metrics
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ServiceMetrics {
    /// CPU usage percentage
    pub cpu_percent: f64,
    /// Memory usage percentage
    pub memory_percent: f64,
    /// Requests per second
    pub rps: Option<f64>,
}

/// Scale request
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ScaleRequest {
    /// Target replica count
    pub replicas: u32,
}

/// Log query parameters
#[derive(Debug, Deserialize, IntoParams)]
pub struct LogQuery {
    /// Number of lines to return
    #[serde(default = "default_lines")]
    pub lines: u32,
    /// Follow logs (streaming)
    #[serde(default)]
    pub follow: bool,
    /// Filter by container/instance
    pub instance: Option<String>,
}

fn default_lines() -> u32 {
    100
}

/// Container summary for API responses
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ContainerSummary {
    /// Container identifier (service-rep-N)
    pub id: String,
    /// Service name
    pub service: String,
    /// Replica number
    pub replica: u32,
    /// Container state
    pub state: String,
    /// Process ID (if running)
    pub pid: Option<u32>,
    /// Overlay IP (if assigned)
    pub overlay_ip: Option<String>,
    /// Raft node ID of the daemon that owns this container (if known).
    ///
    /// Each daemon tags containers it knows about with its own local node ID.
    /// `None` when the API is running in read-only mode (no local Raft node).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
}

/// Exec request body
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ExecRequest {
    /// Command and arguments to execute
    pub command: Vec<String>,
    /// Optional replica number to target
    #[serde(default)]
    pub replica: Option<u32>,
}

/// Exec response body
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ExecResponse {
    /// Exit code from the command
    pub exit_code: i32,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_summary_serialize() {
        let summary = ServiceSummary {
            name: "api".to_string(),
            deployment: "my-app".to_string(),
            status: "running".to_string(),
            replicas: 3,
            desired_replicas: 3,
            endpoints: vec![ServiceEndpoint {
                name: "http".to_string(),
                protocol: "http".to_string(),
                port: 8080,
                url: None,
            }],
        };
        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("api"));
        assert!(json.contains("my-app"));
    }

    #[test]
    fn test_scale_request_deserialize() {
        let json = r#"{"replicas": 5}"#;
        let request: ScaleRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.replicas, 5);
    }

    #[test]
    fn test_log_query_defaults() {
        let query: LogQuery = serde_json::from_str("{}").unwrap();
        assert_eq!(query.lines, 100);
        assert!(!query.follow);
        assert!(query.instance.is_none());
    }
}
