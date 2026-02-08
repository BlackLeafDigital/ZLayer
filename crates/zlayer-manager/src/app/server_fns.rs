//! Server functions for zlayer-manager Leptos SSR + Hydration
//!
//! These server functions provide data access from Leptos components.
//! The `#[server]` macro automatically:
//! - On the server (SSR): runs the function body with full access to backend services
//! - On the client (hydrate): generates an HTTP call to the server function endpoint

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use crate::api_client::{ApiClientError, ZLayerClient};

// ============================================================================
// Client Helper (SSR only)
// ============================================================================

/// Default API URL if ZLAYER_API_URL is not set
#[cfg(feature = "ssr")]
const DEFAULT_API_URL: &str = "http://localhost:3669";

/// Get a ZLayer API client configured from environment
#[cfg(feature = "ssr")]
fn get_api_client() -> ZLayerClient {
    let base_url = std::env::var("ZLAYER_API_URL").unwrap_or_else(|_| DEFAULT_API_URL.to_string());
    let token = std::env::var("ZLAYER_API_TOKEN").ok();
    ZLayerClient::new(base_url, token)
}

/// Convert `ApiClientError` to `ServerFnError`
#[cfg(feature = "ssr")]
fn api_error_to_server_error(err: &ApiClientError) -> ServerFnError {
    ServerFnError::new(err.to_string())
}

// ============================================================================
// Response Types
// ============================================================================

/// System statistics for the manager dashboard
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SystemStats {
    /// Version of zlayer-manager
    pub version: String,
    /// Total number of nodes in the cluster
    pub total_nodes: u32,
    /// Number of healthy nodes
    pub healthy_nodes: u32,
    /// Total deployments
    pub total_deployments: u32,
    /// Active deployments
    pub active_deployments: u32,
    /// Total CPU usage percentage across cluster
    pub cpu_percent: f64,
    /// Total memory usage percentage across cluster
    pub memory_percent: f64,
    /// Server uptime in seconds
    pub uptime_seconds: u64,
}

/// Node information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    /// Node identifier
    pub id: String,
    /// Node name
    pub name: String,
    /// Node status (e.g., "healthy", "unhealthy", "offline")
    pub status: String,
    /// Node IP address
    pub ip_address: String,
    /// CPU usage percentage
    pub cpu_percent: f64,
    /// Memory usage percentage
    pub memory_percent: f64,
    /// Number of running containers
    pub running_containers: u32,
}

/// Deployment information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Deployment {
    /// Deployment identifier
    pub id: String,
    /// Deployment name
    pub name: String,
    /// Deployment status (e.g., "running", "stopped", "pending")
    pub status: String,
    /// Number of replicas
    pub replicas: u32,
    /// Target replicas
    pub target_replicas: u32,
    /// Created timestamp (ISO 8601)
    pub created_at: String,
    /// Last updated timestamp (ISO 8601)
    pub updated_at: String,
}

/// Service information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Service {
    /// Service name
    pub name: String,
    /// Deployment name
    pub deployment: String,
    /// Service status (e.g., "running", "stopped", "pending")
    pub status: String,
    /// Current replica count
    pub replicas: u32,
    /// Desired replica count
    pub desired_replicas: u32,
}

/// Build status enumeration for UI
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BuildState {
    /// Build is queued
    Pending,
    /// Build is running
    Running,
    /// Build completed successfully
    Complete,
    /// Build failed
    Failed,
}

/// Build information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Build {
    /// Build identifier (UUID)
    pub id: String,
    /// Current build status
    pub status: BuildState,
    /// Image ID (if completed)
    pub image_id: Option<String>,
    /// Error message (if failed)
    pub error: Option<String>,
    /// When the build started (ISO 8601)
    pub started_at: String,
    /// When the build completed (ISO 8601)
    pub completed_at: Option<String>,
}

/// Secret information for UI display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretInfo {
    /// The name/identifier of the secret
    pub name: String,
    /// Unix timestamp when the secret was created
    pub created_at: i64,
    /// Unix timestamp when the secret was last updated
    pub updated_at: i64,
    /// Version number of the secret (incremented on each update)
    pub version: u32,
}

/// Job execution information for UI display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobExecution {
    /// Unique execution ID
    pub id: String,
    /// Name of the job
    pub job_name: String,
    /// Current status (pending, initializing, running, completed, failed, cancelled)
    pub status: String,
    /// When the job started (ISO 8601 format)
    pub started_at: Option<String>,
    /// When the job completed (ISO 8601 format)
    pub completed_at: Option<String>,
    /// Exit code (if completed/failed)
    pub exit_code: Option<i32>,
    /// Captured logs
    pub logs: Option<String>,
    /// How the job was triggered
    pub trigger: String,
    /// Error reason (if failed)
    pub error: Option<String>,
    /// Duration in milliseconds (if completed)
    pub duration_ms: Option<u64>,
}

/// Cron job information for UI display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    /// Job name
    pub name: String,
    /// Cron schedule expression
    pub schedule: String,
    /// Whether the job is enabled
    pub enabled: bool,
    /// When the job last ran (ISO 8601 format)
    pub last_run: Option<String>,
    /// Next scheduled run time (ISO 8601 format)
    pub next_run: Option<String>,
}

// ============================================================================
// Server Functions
// ============================================================================

/// Get system statistics for the manager dashboard
#[server(prefix = "/api/manager")]
pub async fn get_system_stats() -> Result<SystemStats, ServerFnError> {
    let client = get_api_client();

    // Get health info from API
    let health = client
        .health()
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    // Get deployments to count them
    let deployments = client.list_deployments().await.unwrap_or_default();

    // SAFETY: Number of deployments will never exceed u32::MAX in practice
    #[allow(clippy::cast_possible_truncation)]
    let total_deployments = deployments.len() as u32;
    // SAFETY: Number of active deployments will never exceed u32::MAX in practice
    #[allow(clippy::cast_possible_truncation)]
    let active_deployments = deployments.iter().filter(|d| d.status == "running").count() as u32;

    Ok(SystemStats {
        version: health.version,
        // Nodes are not yet exposed via API, so we default to 0
        total_nodes: 0,
        healthy_nodes: 0,
        total_deployments,
        active_deployments,
        // CPU/memory metrics not yet exposed, default to 0
        cpu_percent: 0.0,
        memory_percent: 0.0,
        uptime_seconds: health.uptime_secs.unwrap_or(0),
    })
}

/// Get all nodes in the cluster
#[allow(clippy::unused_async)] // async required by Leptos #[server] macro - no node API yet
#[server(prefix = "/api/manager")]
pub async fn get_nodes() -> Result<Vec<Node>, ServerFnError> {
    // Node API not yet implemented in zlayer-api
    // Return empty list for now
    Ok(vec![])
}

/// Get all deployments
#[server(prefix = "/api/manager")]
pub async fn get_deployments() -> Result<Vec<Deployment>, ServerFnError> {
    let client = get_api_client();

    let summaries = client
        .list_deployments()
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    // Map DeploymentSummary to our Deployment type
    // For full details, we'd need to call get_deployment for each one
    let deployments = summaries
        .into_iter()
        .map(|summary| {
            // SAFETY: service_count will never exceed u32::MAX in practice
            #[allow(clippy::cast_possible_truncation)]
            let replicas = summary.service_count as u32;
            Deployment {
                // Use name as ID since DeploymentSummary doesn't have a separate ID
                id: summary.name.clone(),
                name: summary.name,
                status: summary.status,
                // service_count is available but not replica counts in the summary
                replicas,
                target_replicas: replicas,
                created_at: summary.created_at.clone(),
                // Summary doesn't have updated_at, use created_at as fallback
                updated_at: summary.created_at,
            }
        })
        .collect();

    Ok(deployments)
}

/// Get services for a specific deployment
#[server(GetServices, prefix = "/api/manager")]
pub async fn get_services(deployment: String) -> Result<Vec<Service>, ServerFnError> {
    let client = get_api_client();

    let summaries = client
        .list_services(&deployment)
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    // Map ServiceSummary to our Service type
    let services = summaries
        .into_iter()
        .map(|summary| Service {
            name: summary.name,
            deployment: summary.deployment,
            status: summary.status,
            replicas: summary.replicas,
            desired_replicas: summary.desired_replicas,
        })
        .collect();

    Ok(services)
}

/// Get logs for a specific service
#[server(GetServiceLogs, prefix = "/api/manager")]
pub async fn get_service_logs(
    deployment: String,
    service: String,
    lines: Option<usize>,
) -> Result<String, ServerFnError> {
    let client = get_api_client();

    // SAFETY: lines will never exceed u32::MAX in practice
    #[allow(clippy::cast_possible_truncation)]
    let lines_u32 = lines.map(|l| l as u32);
    let logs = client
        .get_service_logs(&deployment, &service, lines_u32)
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    Ok(logs)
}

// ============================================================================
// Build Server Functions
// ============================================================================

/// Get all builds
#[server(GetBuilds, prefix = "/api/manager")]
pub async fn get_builds() -> Result<Vec<Build>, ServerFnError> {
    let client = get_api_client();

    let build_statuses = client
        .list_builds()
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    // Map BuildStatus to our Build type
    let builds = build_statuses
        .into_iter()
        .map(|bs| Build {
            id: bs.id,
            status: match bs.status {
                crate::api_client::BuildStateEnum::Pending => BuildState::Pending,
                crate::api_client::BuildStateEnum::Running => BuildState::Running,
                crate::api_client::BuildStateEnum::Complete => BuildState::Complete,
                crate::api_client::BuildStateEnum::Failed => BuildState::Failed,
            },
            image_id: bs.image_id,
            error: bs.error,
            started_at: bs.started_at,
            completed_at: bs.completed_at,
        })
        .collect();

    Ok(builds)
}

/// Get status for a specific build
#[server(GetBuildStatus, prefix = "/api/manager")]
pub async fn get_build_status(id: String) -> Result<Build, ServerFnError> {
    let client = get_api_client();

    let bs = client
        .get_build_status(&id)
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    Ok(Build {
        id: bs.id,
        status: match bs.status {
            crate::api_client::BuildStateEnum::Pending => BuildState::Pending,
            crate::api_client::BuildStateEnum::Running => BuildState::Running,
            crate::api_client::BuildStateEnum::Complete => BuildState::Complete,
            crate::api_client::BuildStateEnum::Failed => BuildState::Failed,
        },
        image_id: bs.image_id,
        error: bs.error,
        started_at: bs.started_at,
        completed_at: bs.completed_at,
    })
}

/// Get logs for a specific build
#[server(GetBuildLogs, prefix = "/api/manager")]
pub async fn get_build_logs(id: String) -> Result<String, ServerFnError> {
    let client = get_api_client();

    let logs = client
        .get_build_logs(&id)
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    Ok(logs)
}

// ============================================================================
// Secrets Server Functions
// ============================================================================

/// List all secrets
#[server(GetSecrets, prefix = "/api/manager")]
pub async fn get_secrets() -> Result<Vec<SecretInfo>, ServerFnError> {
    let client = get_api_client();
    let secrets = client
        .list_secrets()
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    Ok(secrets
        .into_iter()
        .map(|s| SecretInfo {
            name: s.name,
            created_at: s.created_at,
            updated_at: s.updated_at,
            version: s.version,
        })
        .collect())
}

/// Create a new secret
#[server(CreateSecret, prefix = "/api/manager")]
pub async fn create_secret(name: String, value: String) -> Result<SecretInfo, ServerFnError> {
    let client = get_api_client();
    let result = client
        .create_secret(&name, &value)
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    Ok(SecretInfo {
        name: result.name,
        created_at: result.created_at,
        updated_at: result.updated_at,
        version: result.version,
    })
}

/// Delete a secret
#[server(DeleteSecret, prefix = "/api/manager")]
pub async fn delete_secret(name: String) -> Result<(), ServerFnError> {
    let client = get_api_client();
    client
        .delete_secret(&name)
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    Ok(())
}

// ============================================================================
// Jobs Server Functions
// ============================================================================

/// Trigger a job execution
#[server(TriggerJob, prefix = "/api/manager")]
pub async fn trigger_job(name: String) -> Result<String, ServerFnError> {
    let client = get_api_client();
    let result = client
        .trigger_job(&name)
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    Ok(result.execution_id)
}

/// Get job execution status
#[server(GetJobStatus, prefix = "/api/manager")]
pub async fn get_job_status(execution_id: String) -> Result<JobExecution, ServerFnError> {
    let client = get_api_client();
    let result = client
        .get_execution_status(&execution_id)
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    Ok(JobExecution {
        id: result.id,
        job_name: result.job_name,
        status: result.status,
        started_at: result.started_at,
        completed_at: result.completed_at,
        exit_code: result.exit_code,
        logs: result.logs,
        trigger: result.trigger,
        error: result.error,
        duration_ms: result.duration_ms,
    })
}

/// List job executions
#[server(ListJobExecutions, prefix = "/api/manager")]
pub async fn list_job_executions(
    job_name: String,
    limit: Option<usize>,
) -> Result<Vec<JobExecution>, ServerFnError> {
    let client = get_api_client();
    let executions = client
        .list_job_executions(&job_name, limit, None)
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    Ok(executions
        .into_iter()
        .map(|e| JobExecution {
            id: e.id,
            job_name: e.job_name,
            status: e.status,
            started_at: e.started_at,
            completed_at: e.completed_at,
            exit_code: e.exit_code,
            logs: e.logs,
            trigger: e.trigger,
            error: e.error,
            duration_ms: e.duration_ms,
        })
        .collect())
}

// ============================================================================
// Cron Server Functions
// ============================================================================

/// List all cron jobs
#[server(GetCronJobs, prefix = "/api/manager")]
pub async fn get_cron_jobs() -> Result<Vec<CronJob>, ServerFnError> {
    let client = get_api_client();
    let jobs = client
        .list_cron_jobs()
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    Ok(jobs
        .into_iter()
        .map(|j| CronJob {
            name: j.name,
            schedule: j.schedule,
            enabled: j.enabled,
            last_run: j.last_run,
            next_run: j.next_run,
        })
        .collect())
}

/// Trigger a cron job manually
#[server(TriggerCronJob, prefix = "/api/manager")]
pub async fn trigger_cron_job(name: String) -> Result<String, ServerFnError> {
    let client = get_api_client();
    let result = client
        .trigger_cron_job(&name)
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    Ok(result.execution_id)
}

/// Enable a cron job
#[server(EnableCronJob, prefix = "/api/manager")]
pub async fn enable_cron_job(name: String) -> Result<(), ServerFnError> {
    let client = get_api_client();
    client
        .enable_cron_job(&name)
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    Ok(())
}

/// Disable a cron job
#[server(DisableCronJob, prefix = "/api/manager")]
pub async fn disable_cron_job(name: String) -> Result<(), ServerFnError> {
    let client = get_api_client();
    client
        .disable_cron_job(&name)
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    Ok(())
}

// ============================================================================
// Overlay Types
// ============================================================================

/// Overlay peer information for UI
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayPeer {
    pub public_key: String,
    pub overlay_ip: Option<String>,
    pub healthy: bool,
    pub last_handshake_secs: Option<u64>,
    pub last_ping_ms: Option<u64>,
}

/// Overlay status for UI
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayStatus {
    pub interface: String,
    pub is_leader: bool,
    pub node_ip: String,
    pub cidr: String,
    pub total_peers: usize,
    pub healthy_peers: usize,
}

/// IP allocation info for UI
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpAllocation {
    pub cidr: String,
    pub total_ips: u32,
    pub allocated_count: usize,
    pub available_count: u32,
}

/// DNS status for UI
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsStatus {
    pub enabled: bool,
    pub zone: Option<String>,
    pub service_count: usize,
    pub services: Vec<String>,
}

// ============================================================================
// Overlay Server Functions
// ============================================================================

/// Get overlay network status
#[server(GetOverlayStatus, prefix = "/api/manager")]
pub async fn get_overlay_status() -> Result<OverlayStatus, ServerFnError> {
    let client = get_api_client();
    let status = client
        .get_overlay_status()
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    Ok(OverlayStatus {
        interface: status.interface,
        is_leader: status.is_leader,
        node_ip: status.node_ip,
        cidr: status.cidr,
        total_peers: status.total_peers,
        healthy_peers: status.healthy_peers,
    })
}

/// Get overlay peers
#[server(GetOverlayPeers, prefix = "/api/manager")]
pub async fn get_overlay_peers() -> Result<Vec<OverlayPeer>, ServerFnError> {
    let client = get_api_client();
    let response = client
        .get_overlay_peers()
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    Ok(response
        .peers
        .into_iter()
        .map(|p| OverlayPeer {
            public_key: p.public_key,
            overlay_ip: p.overlay_ip,
            healthy: p.healthy,
            last_handshake_secs: p.last_handshake_secs,
            last_ping_ms: p.last_ping_ms,
        })
        .collect())
}

/// Get IP allocation status
#[server(GetIpAllocation, prefix = "/api/manager")]
pub async fn get_ip_allocation() -> Result<IpAllocation, ServerFnError> {
    let client = get_api_client();
    let response = client
        .get_ip_allocation()
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    Ok(IpAllocation {
        cidr: response.cidr,
        total_ips: response.total_ips,
        allocated_count: response.allocated_count,
        available_count: response.available_count,
    })
}

/// Get DNS service status
#[server(GetDnsStatus, prefix = "/api/manager")]
pub async fn get_dns_status() -> Result<DnsStatus, ServerFnError> {
    let client = get_api_client();
    let response = client
        .get_dns_status()
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    Ok(DnsStatus {
        enabled: response.enabled,
        zone: response.zone,
        service_count: response.service_count,
        services: response.services,
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // SystemStats Tests
    // =========================================================================

    #[test]
    fn test_system_stats_default() {
        let stats = SystemStats::default();
        assert!(stats.version.is_empty());
        assert_eq!(stats.total_nodes, 0);
        assert_eq!(stats.healthy_nodes, 0);
        assert_eq!(stats.total_deployments, 0);
        assert_eq!(stats.active_deployments, 0);
        assert!((stats.cpu_percent - 0.0).abs() < f64::EPSILON);
        assert!((stats.memory_percent - 0.0).abs() < f64::EPSILON);
        assert_eq!(stats.uptime_seconds, 0);
    }

    #[test]
    fn test_system_stats_serialize() {
        let stats = SystemStats {
            version: "1.0.0".to_string(),
            total_nodes: 5,
            healthy_nodes: 4,
            total_deployments: 10,
            active_deployments: 8,
            cpu_percent: 45.5,
            memory_percent: 60.2,
            uptime_seconds: 3600,
        };
        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("1.0.0"));
        assert!(json.contains("\"total_nodes\":5"));
        assert!(json.contains("45.5"));
    }

    #[test]
    fn test_system_stats_deserialize() {
        let json = r#"{
            "version": "2.0.0",
            "total_nodes": 10,
            "healthy_nodes": 9,
            "total_deployments": 20,
            "active_deployments": 15,
            "cpu_percent": 30.0,
            "memory_percent": 50.0,
            "uptime_seconds": 7200
        }"#;
        let stats: SystemStats = serde_json::from_str(json).unwrap();
        assert_eq!(stats.version, "2.0.0");
        assert_eq!(stats.total_nodes, 10);
        assert_eq!(stats.healthy_nodes, 9);
        assert_eq!(stats.active_deployments, 15);
    }

    // =========================================================================
    // Node Tests
    // =========================================================================

    #[test]
    fn test_node_serialize() {
        let node = Node {
            id: "node-1".to_string(),
            name: "worker-1".to_string(),
            status: "healthy".to_string(),
            ip_address: "192.168.1.10".to_string(),
            cpu_percent: 25.5,
            memory_percent: 40.0,
            running_containers: 5,
        };
        let json = serde_json::to_string(&node).unwrap();
        assert!(json.contains("node-1"));
        assert!(json.contains("worker-1"));
        assert!(json.contains("healthy"));
        assert!(json.contains("192.168.1.10"));
    }

    #[test]
    fn test_node_deserialize() {
        let json = r#"{
            "id": "node-2",
            "name": "worker-2",
            "status": "unhealthy",
            "ip_address": "192.168.1.11",
            "cpu_percent": 80.0,
            "memory_percent": 90.0,
            "running_containers": 10
        }"#;
        let node: Node = serde_json::from_str(json).unwrap();
        assert_eq!(node.id, "node-2");
        assert_eq!(node.status, "unhealthy");
        assert_eq!(node.running_containers, 10);
    }

    // =========================================================================
    // Deployment Tests
    // =========================================================================

    #[test]
    fn test_deployment_serialize() {
        let deployment = Deployment {
            id: "dep-1".to_string(),
            name: "my-app".to_string(),
            status: "running".to_string(),
            replicas: 3,
            target_replicas: 3,
            created_at: "2025-01-01T00:00:00Z".to_string(),
            updated_at: "2025-01-02T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&deployment).unwrap();
        assert!(json.contains("dep-1"));
        assert!(json.contains("my-app"));
        assert!(json.contains("running"));
    }

    #[test]
    fn test_deployment_deserialize() {
        let json = r#"{
            "id": "dep-2",
            "name": "api-service",
            "status": "stopped",
            "replicas": 0,
            "target_replicas": 2,
            "created_at": "2025-01-01T00:00:00Z",
            "updated_at": "2025-01-03T00:00:00Z"
        }"#;
        let deployment: Deployment = serde_json::from_str(json).unwrap();
        assert_eq!(deployment.id, "dep-2");
        assert_eq!(deployment.name, "api-service");
        assert_eq!(deployment.replicas, 0);
        assert_eq!(deployment.target_replicas, 2);
    }

    // =========================================================================
    // Service Tests
    // =========================================================================

    #[test]
    fn test_service_serialize() {
        let service = Service {
            name: "web".to_string(),
            deployment: "my-app".to_string(),
            status: "running".to_string(),
            replicas: 2,
            desired_replicas: 3,
        };
        let json = serde_json::to_string(&service).unwrap();
        assert!(json.contains("web"));
        assert!(json.contains("my-app"));
    }

    #[test]
    fn test_service_deserialize() {
        let json = r#"{
            "name": "db",
            "deployment": "backend",
            "status": "pending",
            "replicas": 0,
            "desired_replicas": 1
        }"#;
        let service: Service = serde_json::from_str(json).unwrap();
        assert_eq!(service.name, "db");
        assert_eq!(service.deployment, "backend");
        assert_eq!(service.status, "pending");
    }

    // =========================================================================
    // BuildState Tests
    // =========================================================================

    #[test]
    fn test_build_state_serialize() {
        assert_eq!(
            serde_json::to_string(&BuildState::Pending).unwrap(),
            "\"pending\""
        );
        assert_eq!(
            serde_json::to_string(&BuildState::Running).unwrap(),
            "\"running\""
        );
        assert_eq!(
            serde_json::to_string(&BuildState::Complete).unwrap(),
            "\"complete\""
        );
        assert_eq!(
            serde_json::to_string(&BuildState::Failed).unwrap(),
            "\"failed\""
        );
    }

    #[test]
    fn test_build_state_deserialize() {
        assert_eq!(
            serde_json::from_str::<BuildState>("\"pending\"").unwrap(),
            BuildState::Pending
        );
        assert_eq!(
            serde_json::from_str::<BuildState>("\"running\"").unwrap(),
            BuildState::Running
        );
        assert_eq!(
            serde_json::from_str::<BuildState>("\"complete\"").unwrap(),
            BuildState::Complete
        );
        assert_eq!(
            serde_json::from_str::<BuildState>("\"failed\"").unwrap(),
            BuildState::Failed
        );
    }

    // =========================================================================
    // Build Tests
    // =========================================================================

    #[test]
    fn test_build_serialize() {
        let build = Build {
            id: "build-123".to_string(),
            status: BuildState::Complete,
            image_id: Some("sha256:abc123".to_string()),
            error: None,
            started_at: "2025-01-01T12:00:00Z".to_string(),
            completed_at: Some("2025-01-01T12:05:00Z".to_string()),
        };
        let json = serde_json::to_string(&build).unwrap();
        assert!(json.contains("build-123"));
        assert!(json.contains("complete"));
        assert!(json.contains("sha256:abc123"));
    }

    #[test]
    fn test_build_deserialize_complete() {
        let json = r#"{
            "id": "build-456",
            "status": "complete",
            "image_id": "sha256:def456",
            "started_at": "2025-01-01T12:00:00Z",
            "completed_at": "2025-01-01T12:10:00Z"
        }"#;
        let build: Build = serde_json::from_str(json).unwrap();
        assert_eq!(build.id, "build-456");
        assert_eq!(build.status, BuildState::Complete);
        assert_eq!(build.image_id, Some("sha256:def456".to_string()));
        assert!(build.error.is_none());
    }

    #[test]
    fn test_build_deserialize_failed() {
        let json = r#"{
            "id": "build-789",
            "status": "failed",
            "error": "Compilation failed",
            "started_at": "2025-01-01T12:00:00Z",
            "completed_at": "2025-01-01T12:01:00Z"
        }"#;
        let build: Build = serde_json::from_str(json).unwrap();
        assert_eq!(build.id, "build-789");
        assert_eq!(build.status, BuildState::Failed);
        assert!(build.image_id.is_none());
        assert_eq!(build.error, Some("Compilation failed".to_string()));
    }

    // =========================================================================
    // SecretInfo Tests
    // =========================================================================

    #[test]
    fn test_secret_info_serialize() {
        let secret = SecretInfo {
            name: "api-key".to_string(),
            created_at: 1_706_745_600,
            updated_at: 1_706_745_700,
            version: 2,
        };
        let json = serde_json::to_string(&secret).unwrap();
        assert!(json.contains("api-key"));
        assert!(json.contains("1706745600"));
        assert!(json.contains("\"version\":2"));
    }

    #[test]
    fn test_secret_info_deserialize() {
        let json = r#"{
            "name": "db-password",
            "created_at": 1706745000,
            "updated_at": 1706745500,
            "version": 5
        }"#;
        let secret: SecretInfo = serde_json::from_str(json).unwrap();
        assert_eq!(secret.name, "db-password");
        assert_eq!(secret.created_at, 1_706_745_000);
        assert_eq!(secret.updated_at, 1_706_745_500);
        assert_eq!(secret.version, 5);
    }

    // =========================================================================
    // JobExecution Tests
    // =========================================================================

    #[test]
    fn test_job_execution_serialize() {
        let execution = JobExecution {
            id: "exec-123".to_string(),
            job_name: "backup".to_string(),
            status: "completed".to_string(),
            started_at: Some("2025-01-01T12:00:00Z".to_string()),
            completed_at: Some("2025-01-01T12:05:00Z".to_string()),
            exit_code: Some(0),
            logs: Some("Backup completed successfully".to_string()),
            trigger: "manual".to_string(),
            error: None,
            duration_ms: Some(300_000),
        };
        let json = serde_json::to_string(&execution).unwrap();
        assert!(json.contains("exec-123"));
        assert!(json.contains("backup"));
        assert!(json.contains("completed"));
        assert!(json.contains("300000"));
    }

    #[test]
    fn test_job_execution_deserialize_full() {
        let json = r#"{
            "id": "exec-456",
            "job_name": "sync",
            "status": "failed",
            "started_at": "2025-01-01T12:00:00Z",
            "completed_at": "2025-01-01T12:01:00Z",
            "exit_code": 1,
            "logs": "Error occurred",
            "trigger": "cron",
            "error": "Connection timeout",
            "duration_ms": 60000
        }"#;
        let execution: JobExecution = serde_json::from_str(json).unwrap();
        assert_eq!(execution.id, "exec-456");
        assert_eq!(execution.job_name, "sync");
        assert_eq!(execution.status, "failed");
        assert_eq!(execution.exit_code, Some(1));
        assert_eq!(execution.error, Some("Connection timeout".to_string()));
    }

    #[test]
    fn test_job_execution_deserialize_minimal() {
        let json = r#"{
            "id": "exec-789",
            "job_name": "test",
            "status": "pending",
            "trigger": "api"
        }"#;
        let execution: JobExecution = serde_json::from_str(json).unwrap();
        assert_eq!(execution.id, "exec-789");
        assert_eq!(execution.status, "pending");
        assert!(execution.started_at.is_none());
        assert!(execution.completed_at.is_none());
        assert!(execution.exit_code.is_none());
        assert!(execution.logs.is_none());
        assert!(execution.error.is_none());
        assert!(execution.duration_ms.is_none());
    }

    // =========================================================================
    // CronJob Tests
    // =========================================================================

    #[test]
    fn test_cron_job_serialize() {
        let cron = CronJob {
            name: "daily-backup".to_string(),
            schedule: "0 0 * * *".to_string(),
            enabled: true,
            last_run: Some("2025-01-01T00:00:00Z".to_string()),
            next_run: Some("2025-01-02T00:00:00Z".to_string()),
        };
        let json = serde_json::to_string(&cron).unwrap();
        assert!(json.contains("daily-backup"));
        assert!(json.contains("0 0 * * *"));
        assert!(json.contains("\"enabled\":true"));
    }

    #[test]
    fn test_cron_job_deserialize_full() {
        let json = r#"{
            "name": "hourly-sync",
            "schedule": "0 * * * *",
            "enabled": true,
            "last_run": "2025-01-01T11:00:00Z",
            "next_run": "2025-01-01T12:00:00Z"
        }"#;
        let cron: CronJob = serde_json::from_str(json).unwrap();
        assert_eq!(cron.name, "hourly-sync");
        assert_eq!(cron.schedule, "0 * * * *");
        assert!(cron.enabled);
        assert!(cron.last_run.is_some());
        assert!(cron.next_run.is_some());
    }

    #[test]
    fn test_cron_job_deserialize_minimal() {
        let json = r#"{
            "name": "disabled-job",
            "schedule": "0 0 * * 0",
            "enabled": false
        }"#;
        let cron: CronJob = serde_json::from_str(json).unwrap();
        assert_eq!(cron.name, "disabled-job");
        assert!(!cron.enabled);
        assert!(cron.last_run.is_none());
        assert!(cron.next_run.is_none());
    }

    // =========================================================================
    // OverlayPeer Tests
    // =========================================================================

    #[test]
    fn test_overlay_peer_serialize() {
        let peer = OverlayPeer {
            public_key: "abc123=".to_string(),
            overlay_ip: Some("10.0.0.5".to_string()),
            healthy: true,
            last_handshake_secs: Some(30),
            last_ping_ms: Some(5),
        };
        let json = serde_json::to_string(&peer).unwrap();
        assert!(json.contains("abc123="));
        assert!(json.contains("10.0.0.5"));
        assert!(json.contains("\"healthy\":true"));
    }

    #[test]
    fn test_overlay_peer_deserialize() {
        let json = r#"{
            "public_key": "xyz789=",
            "overlay_ip": "10.0.0.10",
            "healthy": false,
            "last_handshake_secs": 120,
            "last_ping_ms": 15
        }"#;
        let peer: OverlayPeer = serde_json::from_str(json).unwrap();
        assert_eq!(peer.public_key, "xyz789=");
        assert_eq!(peer.overlay_ip, Some("10.0.0.10".to_string()));
        assert!(!peer.healthy);
        assert_eq!(peer.last_handshake_secs, Some(120));
        assert_eq!(peer.last_ping_ms, Some(15));
    }

    #[test]
    fn test_overlay_peer_deserialize_minimal() {
        let json = r#"{
            "public_key": "minimal=",
            "healthy": true
        }"#;
        let peer: OverlayPeer = serde_json::from_str(json).unwrap();
        assert_eq!(peer.public_key, "minimal=");
        assert!(peer.overlay_ip.is_none());
        assert!(peer.healthy);
        assert!(peer.last_handshake_secs.is_none());
        assert!(peer.last_ping_ms.is_none());
    }

    // =========================================================================
    // OverlayStatus Tests
    // =========================================================================

    #[test]
    fn test_overlay_status_serialize() {
        let status = OverlayStatus {
            interface: "wg0".to_string(),
            is_leader: true,
            node_ip: "10.0.0.1".to_string(),
            cidr: "10.0.0.0/24".to_string(),
            total_peers: 5,
            healthy_peers: 4,
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("wg0"));
        assert!(json.contains("10.0.0.1"));
        assert!(json.contains("\"is_leader\":true"));
        assert!(json.contains("\"total_peers\":5"));
    }

    #[test]
    fn test_overlay_status_deserialize() {
        let json = r#"{
            "interface": "wg1",
            "is_leader": false,
            "node_ip": "10.0.0.2",
            "cidr": "10.0.0.0/16",
            "total_peers": 10,
            "healthy_peers": 8
        }"#;
        let status: OverlayStatus = serde_json::from_str(json).unwrap();
        assert_eq!(status.interface, "wg1");
        assert!(!status.is_leader);
        assert_eq!(status.node_ip, "10.0.0.2");
        assert_eq!(status.cidr, "10.0.0.0/16");
        assert_eq!(status.total_peers, 10);
        assert_eq!(status.healthy_peers, 8);
    }

    // =========================================================================
    // IpAllocation Tests
    // =========================================================================

    #[test]
    fn test_ip_allocation_serialize() {
        let alloc = IpAllocation {
            cidr: "10.0.0.0/24".to_string(),
            total_ips: 254,
            allocated_count: 50,
            available_count: 204,
        };
        let json = serde_json::to_string(&alloc).unwrap();
        assert!(json.contains("10.0.0.0/24"));
        assert!(json.contains("254"));
        assert!(json.contains("\"allocated_count\":50"));
    }

    #[test]
    fn test_ip_allocation_deserialize() {
        let json = r#"{
            "cidr": "192.168.0.0/16",
            "total_ips": 65534,
            "allocated_count": 100,
            "available_count": 65434
        }"#;
        let alloc: IpAllocation = serde_json::from_str(json).unwrap();
        assert_eq!(alloc.cidr, "192.168.0.0/16");
        assert_eq!(alloc.total_ips, 65534);
        assert_eq!(alloc.allocated_count, 100);
        assert_eq!(alloc.available_count, 65434);
    }

    // =========================================================================
    // DnsStatus Tests
    // =========================================================================

    #[test]
    fn test_dns_status_serialize() {
        let dns = DnsStatus {
            enabled: true,
            zone: Some("zlayer.local".to_string()),
            service_count: 3,
            services: vec!["api".to_string(), "web".to_string(), "db".to_string()],
        };
        let json = serde_json::to_string(&dns).unwrap();
        assert!(json.contains("\"enabled\":true"));
        assert!(json.contains("zlayer.local"));
        assert!(json.contains("api"));
        assert!(json.contains("web"));
        assert!(json.contains("db"));
    }

    #[test]
    fn test_dns_status_deserialize_enabled() {
        let json = r#"{
            "enabled": true,
            "zone": "test.local",
            "service_count": 2,
            "services": ["svc1", "svc2"]
        }"#;
        let dns: DnsStatus = serde_json::from_str(json).unwrap();
        assert!(dns.enabled);
        assert_eq!(dns.zone, Some("test.local".to_string()));
        assert_eq!(dns.service_count, 2);
        assert_eq!(dns.services.len(), 2);
    }

    #[test]
    fn test_dns_status_deserialize_disabled() {
        let json = r#"{
            "enabled": false,
            "service_count": 0,
            "services": []
        }"#;
        let dns: DnsStatus = serde_json::from_str(json).unwrap();
        assert!(!dns.enabled);
        assert!(dns.zone.is_none());
        assert_eq!(dns.service_count, 0);
        assert!(dns.services.is_empty());
    }

    // =========================================================================
    // Round-trip Tests
    // =========================================================================

    #[test]
    fn test_system_stats_roundtrip() {
        let original = SystemStats {
            version: "1.0.0".to_string(),
            total_nodes: 5,
            healthy_nodes: 4,
            total_deployments: 10,
            active_deployments: 8,
            cpu_percent: 45.5,
            memory_percent: 60.2,
            uptime_seconds: 3600,
        };
        let json = serde_json::to_string(&original).unwrap();
        let restored: SystemStats = serde_json::from_str(&json).unwrap();
        assert_eq!(original.version, restored.version);
        assert_eq!(original.total_nodes, restored.total_nodes);
        assert_eq!(original.uptime_seconds, restored.uptime_seconds);
    }

    #[test]
    fn test_secret_info_roundtrip() {
        let original = SecretInfo {
            name: "test-secret".to_string(),
            created_at: 1_706_745_600,
            updated_at: 1_706_745_700,
            version: 3,
        };
        let json = serde_json::to_string(&original).unwrap();
        let restored: SecretInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(original.name, restored.name);
        assert_eq!(original.created_at, restored.created_at);
        assert_eq!(original.version, restored.version);
    }

    #[test]
    fn test_job_execution_roundtrip() {
        let original = JobExecution {
            id: "exec-test".to_string(),
            job_name: "test-job".to_string(),
            status: "running".to_string(),
            started_at: Some("2025-01-01T12:00:00Z".to_string()),
            completed_at: None,
            exit_code: None,
            logs: None,
            trigger: "api".to_string(),
            error: None,
            duration_ms: None,
        };
        let json = serde_json::to_string(&original).unwrap();
        let restored: JobExecution = serde_json::from_str(&json).unwrap();
        assert_eq!(original.id, restored.id);
        assert_eq!(original.job_name, restored.job_name);
        assert_eq!(original.status, restored.status);
    }

    #[test]
    fn test_overlay_status_roundtrip() {
        let original = OverlayStatus {
            interface: "wg0".to_string(),
            is_leader: true,
            node_ip: "10.0.0.1".to_string(),
            cidr: "10.0.0.0/24".to_string(),
            total_peers: 5,
            healthy_peers: 4,
        };
        let json = serde_json::to_string(&original).unwrap();
        let restored: OverlayStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(original.interface, restored.interface);
        assert_eq!(original.is_leader, restored.is_leader);
        assert_eq!(original.total_peers, restored.total_peers);
    }

    #[test]
    fn test_dns_status_roundtrip() {
        let original = DnsStatus {
            enabled: true,
            zone: Some("zlayer.local".to_string()),
            service_count: 3,
            services: vec!["a".to_string(), "b".to_string()],
        };
        let json = serde_json::to_string(&original).unwrap();
        let restored: DnsStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(original.enabled, restored.enabled);
        assert_eq!(original.zone, restored.zone);
        assert_eq!(original.services, restored.services);
    }
}
