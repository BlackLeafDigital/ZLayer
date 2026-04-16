//! HTTP client for calling ZLayer API
//!
//! Provides a simple async client for interacting with the ZLayer API server.
//! Uses reqwest for HTTP operations and supports optional Bearer token authentication.

use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors that can occur when calling the ZLayer API
#[derive(Debug, Error)]
pub enum ApiClientError {
    /// HTTP request failed
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// Server returned an error response
    #[error("API error ({status}): {message}")]
    Api { status: u16, message: String },

    /// Failed to deserialize response
    #[error("Deserialization error: {0}")]
    Deserialize(String),

    /// Resource not found
    #[error("Not found: {0}")]
    NotFound(String),

    /// Authentication failed
    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    /// Rate limited
    #[error("Rate limited")]
    RateLimited,
}

/// Result type for API client operations
pub type Result<T> = std::result::Result<T, ApiClientError>;

/// Health check response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    /// Service status
    pub status: String,
    /// Service version
    pub version: String,
    /// Uptime in seconds (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime_secs: Option<u64>,
    /// Container runtime name (e.g. "youki", "mac-sandbox", "docker")
    #[serde(default = "default_runtime_name")]
    pub runtime_name: String,
}

/// Fallback runtime name when the API does not include the field (older servers).
fn default_runtime_name() -> String {
    "auto".to_string()
}

/// Deployment summary for listing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentSummary {
    /// Deployment name
    pub name: String,
    /// Deployment status
    pub status: String,
    /// Number of services
    pub service_count: usize,
    /// Created timestamp (ISO 8601)
    pub created_at: String,
}

/// Deployment details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentDetails {
    /// Deployment name
    pub name: String,
    /// Deployment status
    pub status: String,
    /// Service names
    pub services: Vec<String>,
    /// Created timestamp (ISO 8601)
    pub created_at: String,
    /// Updated timestamp (ISO 8601)
    pub updated_at: String,
}

/// Service summary for listing
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

/// Service details
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceEndpoint {
    /// Endpoint name
    pub name: String,
    /// Protocol (http, https, tcp, etc.)
    pub protocol: String,
    /// Port number
    pub port: u16,
    /// URL path (if public)
    pub url: Option<String>,
}

/// Service metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceMetrics {
    /// CPU usage percentage
    pub cpu_percent: f64,
    /// Memory usage percentage
    pub memory_percent: f64,
    /// Requests per second (optional)
    pub rps: Option<f64>,
}

/// Build status enumeration
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BuildStateEnum {
    /// Build is queued
    Pending,
    /// Build is running
    Running,
    /// Build completed successfully
    Complete,
    /// Build failed
    Failed,
}

/// Build status response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildStatus {
    /// Unique build ID
    pub id: String,
    /// Current build status
    pub status: BuildStateEnum,
    /// Image ID (if completed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_id: Option<String>,
    /// Error message (if failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// When the build started (ISO 8601)
    pub started_at: String,
    /// When the build completed (ISO 8601)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

/// Request to trigger a new build
#[derive(Debug, Serialize)]
pub struct TriggerBuildRequest {
    /// Path to the build context on the server
    pub context_path: String,
    /// Tags to apply to the built image
    #[serde(default)]
    pub tags: Vec<String>,
    /// Use runtime template instead of Dockerfile
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    /// Disable cache
    #[serde(default)]
    pub no_cache: bool,
}

/// Response after triggering a build
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerBuildResponse {
    /// The build ID for tracking
    pub build_id: String,
    /// Human-readable message
    pub message: String,
}

// =========================================================================
// Tunnel Types
// =========================================================================

/// Tunnel summary for listing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelSummary {
    /// Unique tunnel identifier
    pub id: String,
    /// Name of the tunnel
    pub name: String,
    /// Current status (active, disconnected, expired, pending)
    pub status: String,
    /// Services this tunnel can expose
    pub services: Vec<String>,
    /// When the tunnel was created (Unix timestamp)
    pub created_at: u64,
    /// When the token expires (Unix timestamp)
    pub expires_at: u64,
    /// Last time the tunnel connected (Unix timestamp, if ever)
    pub last_connected: Option<u64>,
}

/// Request to create a tunnel
#[derive(Debug, Serialize)]
pub struct CreateTunnelRequest {
    /// Name for this tunnel
    pub name: String,
    /// Services this tunnel can expose
    #[serde(default)]
    pub services: Vec<String>,
    /// Time-to-live in seconds
    pub ttl_secs: u64,
}

/// Response after creating a tunnel
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTunnelResponse {
    /// Unique tunnel identifier
    pub id: String,
    /// Name of the tunnel
    pub name: String,
    /// The tunnel token
    pub token: String,
    /// Services this tunnel can expose
    pub services: Vec<String>,
    /// When the token expires (Unix timestamp)
    pub expires_at: u64,
    /// When the tunnel was created (Unix timestamp)
    pub created_at: u64,
}

// =========================================================================
// Network Types
// =========================================================================

/// Summary returned when listing networks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSummary {
    /// Network name
    pub name: String,
    /// Optional description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Number of CIDR ranges
    pub cidr_count: usize,
    /// Number of members
    pub member_count: usize,
    /// Number of access rules
    pub rule_count: usize,
}

/// Full network policy detail
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkPolicyDetail {
    /// Network name
    pub name: String,
    /// Optional description
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// CIDR ranges
    #[serde(default)]
    pub cidrs: Vec<String>,
    /// Members of this network
    #[serde(default)]
    pub members: Vec<NetworkMemberInfo>,
    /// Access rules
    #[serde(default)]
    pub access_rules: Vec<NetworkAccessRuleInfo>,
}

/// A member of a network
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkMemberInfo {
    /// Member identifier
    pub name: String,
    /// Type of member (user, group, node, cidr)
    #[serde(default)]
    pub kind: String,
}

/// An access rule within a network
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkAccessRuleInfo {
    /// Target service name, or "*" for all
    #[serde(default)]
    pub service: String,
    /// Target deployment name, or "*" for all
    #[serde(default)]
    pub deployment: String,
    /// Specific ports, or None for all
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<u16>>,
    /// Allow or Deny
    #[serde(default)]
    pub action: String,
}

/// Request to create a network
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateNetworkRequest {
    /// Network name
    pub name: String,
    /// Optional description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// CIDR ranges
    #[serde(default)]
    pub cidrs: Vec<String>,
    /// Members
    #[serde(default)]
    pub members: Vec<NetworkMemberInfo>,
    /// Access rules
    #[serde(default)]
    pub access_rules: Vec<NetworkAccessRuleInfo>,
}

// =========================================================================
// Secrets Types
// =========================================================================

/// Secret metadata (value is never exposed)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretMetadata {
    /// The name/identifier of the secret
    pub name: String,
    /// Unix timestamp when the secret was created
    pub created_at: i64,
    /// Unix timestamp when the secret was last updated
    pub updated_at: i64,
    /// Version number of the secret (incremented on each update)
    pub version: u32,
}

/// Request to create a secret
#[derive(Debug, Serialize)]
pub struct CreateSecretRequest {
    /// The name of the secret
    pub name: String,
    /// The secret value (will be encrypted at rest)
    pub value: String,
}

// =========================================================================
// Jobs Types
// =========================================================================

/// Response after triggering a job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerJobResponse {
    /// Unique execution ID for tracking
    pub execution_id: String,
    /// Human-readable message
    pub message: String,
}

/// Job execution status response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobExecutionResponse {
    /// Unique execution ID
    pub id: String,
    /// Name of the job
    pub job_name: String,
    /// Current status (pending, initializing, running, completed, failed, cancelled)
    pub status: String,
    /// When the job started (ISO 8601 format)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// When the job completed (ISO 8601 format)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    /// Exit code (if completed/failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Captured logs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs: Option<String>,
    /// How the job was triggered
    pub trigger: String,
    /// Error reason (if failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Duration in milliseconds (if completed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

// =========================================================================
// Cron Types
// =========================================================================

/// Response for cron job information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobResponse {
    /// Job name
    pub name: String,
    /// Cron schedule expression
    pub schedule: String,
    /// Whether the job is enabled
    pub enabled: bool,
    /// When the job last ran (ISO 8601 format)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run: Option<String>,
    /// Next scheduled run time (ISO 8601 format)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run: Option<String>,
}

/// Response after triggering a cron job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerCronResponse {
    /// Execution ID for tracking
    pub execution_id: String,
    /// Human-readable message
    pub message: String,
}

/// Response for enable/disable operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronStatusResponse {
    /// Job name
    pub name: String,
    /// New enabled state
    pub enabled: bool,
    /// Human-readable message
    pub message: String,
}

// =========================================================================
// Overlay Types
// =========================================================================

/// Overlay network status response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayStatusResponse {
    /// Overlay interface name
    pub interface: String,
    /// Whether this node is the overlay leader
    pub is_leader: bool,
    /// This node's IP in the overlay network
    pub node_ip: String,
    /// CIDR of the overlay network
    pub cidr: String,
    /// Overlay listen port (WireGuard protocol)
    pub port: u16,
    /// Total number of peers
    pub total_peers: usize,
    /// Number of healthy peers
    pub healthy_peers: usize,
    /// Number of unhealthy peers
    pub unhealthy_peers: usize,
    /// Unix timestamp of last health check
    pub last_check: u64,
}

/// Peer information from the overlay network
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayPeerInfo {
    /// Overlay public key
    pub public_key: String,
    /// Peer's IP in the overlay network
    pub overlay_ip: Option<String>,
    /// Whether the peer is healthy
    pub healthy: bool,
    /// Seconds since last handshake
    pub last_handshake_secs: Option<u64>,
    /// Last ping latency in milliseconds
    pub last_ping_ms: Option<u64>,
    /// Number of consecutive health check failures
    pub failure_count: u32,
    /// Unix timestamp of last health check
    pub last_check: u64,
}

/// Response containing list of overlay peers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerListResponse {
    /// Total number of peers
    pub total: usize,
    /// Number of healthy peers
    pub healthy: usize,
    /// List of peer information
    pub peers: Vec<OverlayPeerInfo>,
}

/// IP allocation status response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpAllocationResponse {
    /// Network CIDR
    pub cidr: String,
    /// Total IPs available in the network
    pub total_ips: u32,
    /// Number of allocated IPs
    pub allocated_count: usize,
    /// Number of available IPs
    pub available_count: u32,
    /// Utilization percentage (0.0-100.0)
    pub utilization_percent: f64,
}

/// DNS service status response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsStatusResponse {
    /// Whether DNS is enabled
    pub enabled: bool,
    /// DNS zone (e.g., "zlayer.local")
    pub zone: Option<String>,
    /// DNS server port
    pub port: Option<u16>,
    /// Bind address for DNS server
    pub bind_addr: Option<String>,
    /// Number of registered services
    pub service_count: usize,
    /// List of registered service names
    pub services: Vec<String>,
}

// =========================================================================
// Proxy Types
// =========================================================================

/// Summary of a backend in a proxy route or backend group
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyBackendSummary {
    /// Backend address (host:port)
    pub address: String,
    /// Whether the backend is healthy
    pub healthy: bool,
    /// Number of active connections
    pub active_connections: u64,
}

/// A proxy route (L7 HTTP routing rule)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyRoute {
    /// Host to match (e.g., "api.example.com"), if any
    pub host: Option<String>,
    /// Path prefix to match (e.g., "/api")
    pub path_prefix: String,
    /// Whether to strip the matched prefix before forwarding
    pub strip_prefix: bool,
    /// Backends serving this route
    pub backends: Vec<ProxyBackendSummary>,
}

/// A group of backends behind a load-balancer strategy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyBackendGroup {
    /// Service name this group routes to
    pub service: String,
    /// Load-balancing strategy ("round_robin" or "least_connections")
    pub strategy: String,
    /// Backends in this group
    pub backends: Vec<ProxyBackendSummary>,
}

/// TLS certificate metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsCertificate {
    /// Domain the certificate covers
    pub domain: String,
    /// Certificate issuer (e.g., "Let's Encrypt")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    /// When the certificate expires (ISO 8601)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// Whether auto-renewal is enabled
    pub auto_renew: bool,
}

/// A stream proxy (L4 TCP/UDP forwarding rule)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamProxy {
    /// Protocol ("tcp" or "udp")
    pub protocol: String,
    /// Port the proxy listens on
    pub listen_port: u16,
    /// Backends receiving forwarded traffic
    pub backends: Vec<ProxyBackendSummary>,
}

// =========================================================================
// Cluster Types
// =========================================================================

/// Summary of a cluster node from the Raft state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterNodeSummary {
    /// Raft-level ID
    pub id: String,
    /// Network address (Raft RPC address)
    pub address: String,
    /// Advertise address (public IP)
    pub advertise_addr: String,
    /// Current status (e.g. "ready", "draining", "dead")
    pub status: String,
    /// Role in the Raft cluster: "leader", "voter", or "learner"
    pub role: String,
    /// Join mode: "full" or "replicate"
    pub mode: String,
    /// Whether this node is the Raft leader
    pub is_leader: bool,
    /// Overlay network IP assigned to this node
    pub overlay_ip: String,
    /// Total CPU cores on this node
    pub cpu_total: f64,
    /// Current CPU usage (cores)
    pub cpu_used: f64,
    /// Total memory in bytes
    pub memory_total: u64,
    /// Current memory usage in bytes
    pub memory_used: u64,
    /// When the node was registered (Unix timestamp ms)
    pub registered_at: u64,
    /// Last heartbeat timestamp (Unix timestamp ms)
    pub last_heartbeat: u64,
}

/// Error response from the API
#[derive(Debug, Deserialize)]
struct ErrorResponse {
    #[allow(dead_code)]
    error: String,
    message: String,
}

/// HTTP client for calling ZLayer API
#[derive(Debug, Clone)]
pub struct ZLayerClient {
    /// HTTP client
    client: Client,
    /// Base URL for the API (e.g., "http://localhost:9090")
    base_url: String,
    /// Optional Bearer token for authentication
    token: Option<String>,
}

impl ZLayerClient {
    /// Create a new ZLayer API client
    ///
    /// # Arguments
    ///
    /// * `base_url` - Base URL for the API server (e.g., "http://localhost:9090")
    /// * `token` - Optional Bearer token for authentication
    ///
    /// # Panics
    ///
    /// Panics if the HTTP client cannot be created (e.g., TLS backend failure).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zlayer_manager::api_client::ZLayerClient;
    ///
    /// // Without authentication
    /// let client = ZLayerClient::new("http://localhost:9090", None);
    ///
    /// // With authentication
    /// let client = ZLayerClient::new(
    ///     "http://localhost:9090",
    ///     Some("my-jwt-token".to_string()),
    /// );
    /// ```
    #[allow(clippy::needless_pass_by_value)]
    pub fn new(base_url: impl Into<String>, token: Option<String>) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");
        let base_url = base_url.into();

        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
        }
    }

    /// Set or update the authentication token
    pub fn set_token(&mut self, token: Option<String>) {
        self.token = token;
    }

    /// Build a request with optional authorization header
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.client.request(method, &url);

        if let Some(ref token) = self.token {
            req = req.header("Authorization", format!("Bearer {token}"));
        }

        req
    }

    /// Handle API response, converting errors appropriately
    async fn handle_response<T: for<'de> Deserialize<'de>>(
        &self,
        response: reqwest::Response,
    ) -> Result<T> {
        let status = response.status();

        if status.is_success() {
            response
                .json::<T>()
                .await
                .map_err(|e| ApiClientError::Deserialize(e.to_string()))
        } else {
            // Try to parse error response
            let error_text = response.text().await.unwrap_or_default();

            match status {
                StatusCode::NOT_FOUND => Err(ApiClientError::NotFound(error_text)),
                StatusCode::UNAUTHORIZED => Err(ApiClientError::Unauthorized(error_text)),
                StatusCode::TOO_MANY_REQUESTS => Err(ApiClientError::RateLimited),
                _ => {
                    // Try to parse as ErrorResponse
                    if let Ok(err) = serde_json::from_str::<ErrorResponse>(&error_text) {
                        Err(ApiClientError::Api {
                            status: status.as_u16(),
                            message: err.message,
                        })
                    } else {
                        Err(ApiClientError::Api {
                            status: status.as_u16(),
                            message: error_text,
                        })
                    }
                }
            }
        }
    }

    // =========================================================================
    // Health Endpoints
    // =========================================================================

    /// Check API liveness
    ///
    /// Calls GET /health/live to verify the API server is running.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be deserialized.
    pub async fn health_live(&self) -> Result<HealthResponse> {
        let response = self
            .request(reqwest::Method::GET, "/health/live")
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Check API readiness
    ///
    /// Calls GET /health/ready to verify the API server is ready to serve requests.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be deserialized.
    pub async fn health_ready(&self) -> Result<HealthResponse> {
        let response = self
            .request(reqwest::Method::GET, "/health/ready")
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Convenience method to check overall health (uses readiness check)
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be deserialized.
    pub async fn health(&self) -> Result<HealthResponse> {
        self.health_ready().await
    }

    // =========================================================================
    // Deployment Endpoints
    // =========================================================================

    /// List all deployments
    ///
    /// Calls GET /api/v1/deployments to retrieve a list of all deployments.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be deserialized.
    pub async fn list_deployments(&self) -> Result<Vec<DeploymentSummary>> {
        let response = self
            .request(reqwest::Method::GET, "/api/v1/deployments")
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Get deployment details
    ///
    /// Calls GET /api/v1/deployments/{name} to retrieve details for a specific deployment.
    ///
    /// # Arguments
    ///
    /// * `name` - The deployment name
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be deserialized.
    pub async fn get_deployment(&self, name: &str) -> Result<DeploymentDetails> {
        let response = self
            .request(reqwest::Method::GET, &format!("/api/v1/deployments/{name}"))
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Create a new deployment from a YAML spec
    ///
    /// Calls POST /api/v1/deployments to create a deployment from a YAML specification.
    ///
    /// # Arguments
    ///
    /// * `yaml` - The deployment YAML specification
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails, the spec is invalid,
    /// or the response cannot be deserialized.
    pub async fn create_deployment(&self, yaml: &str) -> Result<DeploymentDetails> {
        let body = serde_json::json!({ "spec": yaml });
        let response = self
            .request(reqwest::Method::POST, "/api/v1/deployments")
            .json(&body)
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Delete a deployment
    ///
    /// Calls DELETE /api/v1/deployments/{name} to remove a deployment.
    ///
    /// # Arguments
    ///
    /// * `name` - The deployment name
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the deployment is not found.
    pub async fn delete_deployment(&self, name: &str) -> Result<()> {
        let response = self
            .request(
                reqwest::Method::DELETE,
                &format!("/api/v1/deployments/{name}"),
            )
            .send()
            .await?;

        let status = response.status();

        if status.is_success() {
            Ok(())
        } else {
            let error_text = response.text().await.unwrap_or_default();

            match status {
                StatusCode::NOT_FOUND => Err(ApiClientError::NotFound(error_text)),
                StatusCode::UNAUTHORIZED => Err(ApiClientError::Unauthorized(error_text)),
                _ => Err(ApiClientError::Api {
                    status: status.as_u16(),
                    message: error_text,
                }),
            }
        }
    }

    // =========================================================================
    // Service Endpoints
    // =========================================================================

    /// List services in a deployment
    ///
    /// Calls GET /api/v1/deployments/{deployment}/services to retrieve services.
    ///
    /// # Arguments
    ///
    /// * `deployment` - The deployment name
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be deserialized.
    pub async fn list_services(&self, deployment: &str) -> Result<Vec<ServiceSummary>> {
        let response = self
            .request(
                reqwest::Method::GET,
                &format!("/api/v1/deployments/{deployment}/services"),
            )
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Get service details
    ///
    /// Calls GET /api/v1/deployments/{deployment}/services/{service} to retrieve details.
    ///
    /// # Arguments
    ///
    /// * `deployment` - The deployment name
    /// * `service` - The service name
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be deserialized.
    pub async fn get_service(&self, deployment: &str, service: &str) -> Result<ServiceDetails> {
        let response = self
            .request(
                reqwest::Method::GET,
                &format!("/api/v1/deployments/{deployment}/services/{service}"),
            )
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Get service logs
    ///
    /// Calls GET /api/v1/deployments/{deployment}/services/{service}/logs to retrieve logs.
    ///
    /// # Arguments
    ///
    /// * `deployment` - The deployment name
    /// * `service` - The service name
    /// * `lines` - Optional number of lines to retrieve (default: 100)
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be deserialized.
    pub async fn get_service_logs(
        &self,
        deployment: &str,
        service: &str,
        lines: Option<u32>,
    ) -> Result<String> {
        let mut req = self.request(
            reqwest::Method::GET,
            &format!("/api/v1/deployments/{deployment}/services/{service}/logs"),
        );

        if let Some(lines) = lines {
            req = req.query(&[("lines", lines)]);
        }

        let response = req.send().await?;

        if response.status().is_success() {
            response
                .text()
                .await
                .map_err(|e| ApiClientError::Deserialize(e.to_string()))
        } else {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();

            match status {
                StatusCode::NOT_FOUND => Err(ApiClientError::NotFound(error_text)),
                StatusCode::UNAUTHORIZED => Err(ApiClientError::Unauthorized(error_text)),
                _ => Err(ApiClientError::Api {
                    status: status.as_u16(),
                    message: error_text,
                }),
            }
        }
    }

    // =========================================================================
    // Build Endpoints
    // =========================================================================

    /// List all builds
    ///
    /// Calls GET /api/v1/builds to retrieve a list of all builds.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be deserialized.
    pub async fn list_builds(&self) -> Result<Vec<BuildStatus>> {
        let response = self
            .request(reqwest::Method::GET, "/api/v1/builds")
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Trigger a new build
    ///
    /// Calls POST /api/v1/build/json to start a new build with the given parameters.
    ///
    /// # Arguments
    ///
    /// * `request` - The build request parameters
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be deserialized.
    pub async fn trigger_build(
        &self,
        request: &TriggerBuildRequest,
    ) -> Result<TriggerBuildResponse> {
        let response = self
            .request(reqwest::Method::POST, "/api/v1/build/json")
            .json(request)
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Get build status
    ///
    /// Calls GET /api/v1/build/{id} to retrieve the status of a specific build.
    ///
    /// # Arguments
    ///
    /// * `id` - The build ID (UUID)
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be deserialized.
    pub async fn get_build_status(&self, id: &str) -> Result<BuildStatus> {
        let response = self
            .request(reqwest::Method::GET, &format!("/api/v1/build/{id}"))
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Get build logs
    ///
    /// Calls GET /api/v1/build/{id}/logs to retrieve the logs for a specific build.
    ///
    /// # Arguments
    ///
    /// * `id` - The build ID (UUID)
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be deserialized.
    pub async fn get_build_logs(&self, id: &str) -> Result<String> {
        let response = self
            .request(reqwest::Method::GET, &format!("/api/v1/build/{id}/logs"))
            .send()
            .await?;

        if response.status().is_success() {
            response
                .text()
                .await
                .map_err(|e| ApiClientError::Deserialize(e.to_string()))
        } else {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();

            match status {
                StatusCode::NOT_FOUND => Err(ApiClientError::NotFound(error_text)),
                StatusCode::UNAUTHORIZED => Err(ApiClientError::Unauthorized(error_text)),
                _ => Err(ApiClientError::Api {
                    status: status.as_u16(),
                    message: error_text,
                }),
            }
        }
    }

    // =========================================================================
    // Secrets Endpoints
    // =========================================================================

    /// List all secrets
    ///
    /// Calls GET /api/v1/secrets to retrieve metadata for all secrets.
    /// Secret values are never exposed through this endpoint.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be deserialized.
    pub async fn list_secrets(&self) -> Result<Vec<SecretMetadata>> {
        let response = self
            .request(reqwest::Method::GET, "/api/v1/secrets")
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Get metadata for a specific secret
    ///
    /// Calls GET /api/v1/secrets/{name} to retrieve metadata for a single secret.
    /// The secret value is never exposed through this endpoint.
    ///
    /// # Arguments
    ///
    /// * `name` - The secret name
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails, the secret is not found,
    /// or the response cannot be deserialized.
    pub async fn get_secret_metadata(&self, name: &str) -> Result<SecretMetadata> {
        let response = self
            .request(reqwest::Method::GET, &format!("/api/v1/secrets/{name}"))
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Create or update a secret
    ///
    /// Calls POST /api/v1/secrets to store a new secret or update an existing one.
    /// The secret value is encrypted at rest.
    ///
    /// # Arguments
    ///
    /// * `name` - The secret name
    /// * `value` - The secret value (will be encrypted)
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails, validation fails,
    /// or the response cannot be deserialized.
    pub async fn create_secret(&self, name: &str, value: &str) -> Result<SecretMetadata> {
        let request = CreateSecretRequest {
            name: name.to_string(),
            value: value.to_string(),
        };

        let response = self
            .request(reqwest::Method::POST, "/api/v1/secrets")
            .json(&request)
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Delete a secret
    ///
    /// Calls DELETE /api/v1/secrets/{name} to permanently remove a secret.
    ///
    /// # Arguments
    ///
    /// * `name` - The secret name
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the secret is not found.
    pub async fn delete_secret(&self, name: &str) -> Result<()> {
        let response = self
            .request(reqwest::Method::DELETE, &format!("/api/v1/secrets/{name}"))
            .send()
            .await?;

        let status = response.status();

        if status.is_success() {
            Ok(())
        } else {
            let error_text = response.text().await.unwrap_or_default();

            match status {
                StatusCode::NOT_FOUND => Err(ApiClientError::NotFound(error_text)),
                StatusCode::UNAUTHORIZED => Err(ApiClientError::Unauthorized(error_text)),
                _ => Err(ApiClientError::Api {
                    status: status.as_u16(),
                    message: error_text,
                }),
            }
        }
    }

    // =========================================================================
    // Jobs Endpoints
    // =========================================================================

    /// Trigger a job execution
    ///
    /// Calls POST /api/v1/jobs/{name}/trigger to start a new execution of the specified job.
    /// Returns immediately with an execution ID that can be used to track progress.
    ///
    /// # Arguments
    ///
    /// * `name` - The job name
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails, the job is not found,
    /// or the response cannot be deserialized.
    pub async fn trigger_job(&self, name: &str) -> Result<TriggerJobResponse> {
        let response = self
            .request(
                reqwest::Method::POST,
                &format!("/api/v1/jobs/{name}/trigger"),
            )
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Get job execution status
    ///
    /// Calls GET /api/v1/jobs/{execution_id}/status to retrieve the current status
    /// of a job execution, including logs if available.
    ///
    /// # Arguments
    ///
    /// * `execution_id` - The execution ID returned from trigger_job
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails, the execution is not found,
    /// or the response cannot be deserialized.
    pub async fn get_execution_status(&self, execution_id: &str) -> Result<JobExecutionResponse> {
        let response = self
            .request(
                reqwest::Method::GET,
                &format!("/api/v1/jobs/{execution_id}/status"),
            )
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// List executions for a job
    ///
    /// Calls GET /api/v1/jobs/{name}/executions to retrieve a list of recent executions.
    ///
    /// # Arguments
    ///
    /// * `job_name` - The job name
    /// * `limit` - Optional maximum number of executions to return (default: 50)
    /// * `status` - Optional filter by status (pending, running, completed, failed)
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be deserialized.
    pub async fn list_job_executions(
        &self,
        job_name: &str,
        limit: Option<usize>,
        status: Option<&str>,
    ) -> Result<Vec<JobExecutionResponse>> {
        let mut req = self.request(
            reqwest::Method::GET,
            &format!("/api/v1/jobs/{job_name}/executions"),
        );

        // Build query parameters
        let mut params = Vec::new();
        if let Some(limit) = limit {
            params.push(("limit", limit.to_string()));
        }
        if let Some(status) = status {
            params.push(("status", status.to_string()));
        }

        if !params.is_empty() {
            req = req.query(&params);
        }

        let response = req.send().await?;

        self.handle_response(response).await
    }

    /// Cancel a running job execution
    ///
    /// Calls POST /api/v1/jobs/{execution_id}/cancel to attempt to cancel a running
    /// or pending job execution.
    ///
    /// # Arguments
    ///
    /// * `execution_id` - The execution ID to cancel
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails, the execution is not found,
    /// or the execution is already in a terminal state.
    pub async fn cancel_execution(&self, execution_id: &str) -> Result<JobExecutionResponse> {
        let response = self
            .request(
                reqwest::Method::POST,
                &format!("/api/v1/jobs/{execution_id}/cancel"),
            )
            .send()
            .await?;

        self.handle_response(response).await
    }

    // =========================================================================
    // Cron Endpoints
    // =========================================================================

    /// List all cron jobs
    ///
    /// Calls GET /api/v1/cron to retrieve a list of all registered cron jobs
    /// with their schedule information.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be deserialized.
    pub async fn list_cron_jobs(&self) -> Result<Vec<CronJobResponse>> {
        let response = self
            .request(reqwest::Method::GET, "/api/v1/cron")
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Get cron job details
    ///
    /// Calls GET /api/v1/cron/{name} to retrieve detailed information about a specific cron job.
    ///
    /// # Arguments
    ///
    /// * `name` - The cron job name
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails, the cron job is not found,
    /// or the response cannot be deserialized.
    pub async fn get_cron_job(&self, name: &str) -> Result<CronJobResponse> {
        let response = self
            .request(reqwest::Method::GET, &format!("/api/v1/cron/{name}"))
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Manually trigger a cron job
    ///
    /// Calls POST /api/v1/cron/{name}/trigger to trigger an immediate execution
    /// of the cron job, regardless of its schedule.
    ///
    /// # Arguments
    ///
    /// * `name` - The cron job name
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails, the cron job is not found,
    /// or the response cannot be deserialized.
    pub async fn trigger_cron_job(&self, name: &str) -> Result<TriggerCronResponse> {
        let response = self
            .request(
                reqwest::Method::POST,
                &format!("/api/v1/cron/{name}/trigger"),
            )
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Enable a cron job
    ///
    /// Calls PUT /api/v1/cron/{name}/enable to enable a disabled cron job,
    /// allowing it to run on schedule.
    ///
    /// # Arguments
    ///
    /// * `name` - The cron job name
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails, the cron job is not found,
    /// or the response cannot be deserialized.
    pub async fn enable_cron_job(&self, name: &str) -> Result<CronStatusResponse> {
        let response = self
            .request(reqwest::Method::PUT, &format!("/api/v1/cron/{name}/enable"))
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Disable a cron job
    ///
    /// Calls PUT /api/v1/cron/{name}/disable to disable a cron job,
    /// preventing it from running on schedule. The job can still be manually triggered.
    ///
    /// # Arguments
    ///
    /// * `name` - The cron job name
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails, the cron job is not found,
    /// or the response cannot be deserialized.
    pub async fn disable_cron_job(&self, name: &str) -> Result<CronStatusResponse> {
        let response = self
            .request(
                reqwest::Method::PUT,
                &format!("/api/v1/cron/{name}/disable"),
            )
            .send()
            .await?;

        self.handle_response(response).await
    }

    // =========================================================================
    // Tunnel Endpoints
    // =========================================================================

    /// List all tunnels
    ///
    /// Calls GET /api/v1/tunnels to retrieve a list of all configured tunnels.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be deserialized.
    pub async fn list_tunnels(&self) -> Result<Vec<TunnelSummary>> {
        let response = self
            .request(reqwest::Method::GET, "/api/v1/tunnels")
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Create a new tunnel
    ///
    /// Calls POST /api/v1/tunnels to create a new tunnel token.
    ///
    /// # Arguments
    ///
    /// * `request` - The tunnel creation request
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be deserialized.
    pub async fn create_tunnel(
        &self,
        request: &CreateTunnelRequest,
    ) -> Result<CreateTunnelResponse> {
        let response = self
            .request(reqwest::Method::POST, "/api/v1/tunnels")
            .json(request)
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Delete (revoke) a tunnel
    ///
    /// Calls DELETE /api/v1/tunnels/{id} to revoke and remove a tunnel.
    ///
    /// # Arguments
    ///
    /// * `id` - The tunnel ID
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the tunnel is not found.
    pub async fn delete_tunnel(&self, id: &str) -> Result<()> {
        let response = self
            .request(reqwest::Method::DELETE, &format!("/api/v1/tunnels/{id}"))
            .send()
            .await?;

        let status = response.status();

        if status.is_success() {
            Ok(())
        } else {
            let error_text = response.text().await.unwrap_or_default();

            match status {
                StatusCode::NOT_FOUND => Err(ApiClientError::NotFound(error_text)),
                StatusCode::UNAUTHORIZED => Err(ApiClientError::Unauthorized(error_text)),
                _ => Err(ApiClientError::Api {
                    status: status.as_u16(),
                    message: error_text,
                }),
            }
        }
    }

    // =========================================================================
    // Overlay Endpoints
    // =========================================================================

    /// Get overlay network status
    ///
    /// Calls GET /api/v1/overlay/status to retrieve the current status of the
    /// overlay network, including peer health information.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be deserialized.
    pub async fn get_overlay_status(&self) -> Result<OverlayStatusResponse> {
        let response = self
            .request(reqwest::Method::GET, "/api/v1/overlay/status")
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Get overlay network peers
    ///
    /// Calls GET /api/v1/overlay/peers to retrieve a list of all peers
    /// in the overlay network with their health status.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be deserialized.
    pub async fn get_overlay_peers(&self) -> Result<PeerListResponse> {
        let response = self
            .request(reqwest::Method::GET, "/api/v1/overlay/peers")
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Get IP allocation status
    ///
    /// Calls GET /api/v1/overlay/ip-alloc to retrieve the current IP allocation
    /// status of the overlay network.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be deserialized.
    pub async fn get_ip_allocation(&self) -> Result<IpAllocationResponse> {
        let response = self
            .request(reqwest::Method::GET, "/api/v1/overlay/ip-alloc")
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Get DNS service status
    ///
    /// Calls GET /api/v1/overlay/dns to retrieve the current status of the
    /// overlay DNS service.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be deserialized.
    pub async fn get_dns_status(&self) -> Result<DnsStatusResponse> {
        let response = self
            .request(reqwest::Method::GET, "/api/v1/overlay/dns")
            .send()
            .await?;

        self.handle_response(response).await
    }

    // =========================================================================
    // Cluster Node Endpoints
    // =========================================================================

    /// List all nodes in the cluster
    ///
    /// Calls GET /api/v1/cluster/nodes to retrieve a list of all nodes
    /// visible in the Raft cluster state.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be deserialized.
    pub async fn list_cluster_nodes(&self) -> Result<Vec<ClusterNodeSummary>> {
        let response = self
            .request(reqwest::Method::GET, "/api/v1/cluster/nodes")
            .send()
            .await?;

        self.handle_response(response).await
    }

    // =========================================================================
    // Proxy Endpoints
    // =========================================================================

    /// List all proxy routes
    ///
    /// Calls GET /api/v1/proxy/routes to retrieve L7 HTTP routing rules.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be deserialized.
    pub async fn list_proxy_routes(&self) -> Result<Vec<ProxyRoute>> {
        let response = self
            .request(reqwest::Method::GET, "/api/v1/proxy/routes")
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// List all proxy backend groups
    ///
    /// Calls GET /api/v1/proxy/backends to retrieve backend groups
    /// and their load-balancing configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be deserialized.
    pub async fn list_proxy_backends(&self) -> Result<Vec<ProxyBackendGroup>> {
        let response = self
            .request(reqwest::Method::GET, "/api/v1/proxy/backends")
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// List all TLS certificates
    ///
    /// Calls GET /api/v1/proxy/tls to retrieve TLS certificate metadata.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be deserialized.
    pub async fn list_tls_certificates(&self) -> Result<Vec<TlsCertificate>> {
        let response = self
            .request(reqwest::Method::GET, "/api/v1/proxy/tls")
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// List all stream proxies
    ///
    /// Calls GET /api/v1/proxy/streams to retrieve L4 TCP/UDP stream proxy rules.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be deserialized.
    pub async fn list_stream_proxies(&self) -> Result<Vec<StreamProxy>> {
        let response = self
            .request(reqwest::Method::GET, "/api/v1/proxy/streams")
            .send()
            .await?;

        self.handle_response(response).await
    }

    // =========================================================================
    // Network Endpoints
    // =========================================================================

    /// List all networks
    ///
    /// Calls GET /api/v1/networks to retrieve a summary of all defined networks.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be deserialized.
    pub async fn list_networks(&self) -> Result<Vec<NetworkSummary>> {
        let response = self
            .request(reqwest::Method::GET, "/api/v1/networks")
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Get a specific network by name
    ///
    /// Calls GET /api/v1/networks/{name} to retrieve the full network policy.
    ///
    /// # Arguments
    ///
    /// * `name` - The network name
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails, the network is not found,
    /// or the response cannot be deserialized.
    pub async fn get_network(&self, name: &str) -> Result<NetworkPolicyDetail> {
        let response = self
            .request(reqwest::Method::GET, &format!("/api/v1/networks/{name}"))
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Create a new network
    ///
    /// Calls POST /api/v1/networks to create a new network policy.
    ///
    /// # Arguments
    ///
    /// * `request` - The network creation request
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails, the network already exists,
    /// or the response cannot be deserialized.
    pub async fn create_network(
        &self,
        request: &CreateNetworkRequest,
    ) -> Result<NetworkPolicyDetail> {
        let response = self
            .request(reqwest::Method::POST, "/api/v1/networks")
            .json(request)
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Delete a network
    ///
    /// Calls DELETE /api/v1/networks/{name} to remove a network policy.
    ///
    /// # Arguments
    ///
    /// * `name` - The network name to delete
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the network is not found.
    pub async fn delete_network(&self, name: &str) -> Result<()> {
        let response = self
            .request(reqwest::Method::DELETE, &format!("/api/v1/networks/{name}"))
            .send()
            .await?;

        let status = response.status();

        if status.is_success() {
            Ok(())
        } else {
            let error_text = response.text().await.unwrap_or_default();

            match status {
                StatusCode::NOT_FOUND => Err(ApiClientError::NotFound(error_text)),
                StatusCode::UNAUTHORIZED => Err(ApiClientError::Unauthorized(error_text)),
                _ => Err(ApiClientError::Api {
                    status: status.as_u16(),
                    message: error_text,
                }),
            }
        }
    }

    // ---- Raw auth passthrough ----

    /// Perform a raw HTTP request against the underlying transport and return
    /// the status, body, and `Set-Cookie` headers intact.
    ///
    /// Used by Manager server_fns to proxy auth and users endpoints. Unlike the
    /// typed methods, this one forwards the caller's cookies and preserves the
    /// upstream's response cookies so the browser session round-trips cleanly.
    ///
    /// # Arguments
    /// - `method` — HTTP method.
    /// - `path` — path-and-query segment starting with `/` (e.g. `/auth/login`
    ///   or `/api/v1/users?x=1`).
    /// - `body` — optional request body (sent as `application/json` when
    ///   `Some`).
    /// - `forward_cookie` — optional `Cookie` header value to forward from the
    ///   browser's request (e.g. `zlayer_session=…; zlayer_csrf=…`).
    /// - `csrf_token` — optional `X-CSRF-Token` header value (double-submit).
    ///
    /// # Errors
    /// Returns `ApiClientError::Http` on transport-level failures. Does NOT
    /// map non-2xx statuses to errors — the caller is expected to inspect
    /// `RawResponse::status`.
    pub async fn raw_request(
        &self,
        method: RawMethod,
        path: &str,
        body: Option<&[u8]>,
        forward_cookie: Option<&str>,
        csrf_token: Option<&str>,
    ) -> Result<RawResponse> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.client.request(method.as_reqwest(), &url);
        if let Some(tok) = &self.token {
            req = req.header(reqwest::header::AUTHORIZATION, format!("Bearer {tok}"));
        }
        if let Some(cookie) = forward_cookie {
            req = req.header(reqwest::header::COOKIE, cookie);
        }
        if let Some(csrf) = csrf_token {
            req = req.header("x-csrf-token", csrf);
        }
        if let Some(body) = body {
            req = req
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_vec());
        }

        let resp = req.send().await?;
        let status = resp.status();
        let set_cookies: Vec<String> = resp
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .map(str::to_string)
            .collect();
        let body = resp.bytes().await?.to_vec();
        Ok(RawResponse {
            status,
            body,
            set_cookies,
        })
    }
}

/// Raw response from a passthrough HTTP call.
///
/// Exposes the status, body bytes, and any `Set-Cookie` headers the upstream
/// API wrote — used by Leptos server_fns to mirror login/logout cookies back
/// to the browser.
#[derive(Debug)]
pub struct RawResponse {
    /// Upstream HTTP status code.
    pub status: reqwest::StatusCode,
    /// Response body bytes.
    pub body: Vec<u8>,
    /// `Set-Cookie` header values from the upstream response, one entry per
    /// header (multi-valued).
    pub set_cookies: Vec<String>,
}

/// HTTP method selector for the raw passthrough. We don't use `reqwest::Method`
/// directly so callers don't need a reqwest dep.
#[derive(Debug, Clone, Copy)]
pub enum RawMethod {
    /// HTTP GET.
    Get,
    /// HTTP POST.
    Post,
    /// HTTP PATCH.
    Patch,
    /// HTTP DELETE.
    Delete,
}

impl RawMethod {
    fn as_reqwest(self) -> reqwest::Method {
        match self {
            RawMethod::Get => reqwest::Method::GET,
            RawMethod::Post => reqwest::Method::POST,
            RawMethod::Patch => reqwest::Method::PATCH,
            RawMethod::Delete => reqwest::Method::DELETE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_new() {
        let client = ZLayerClient::new("http://localhost:9090".to_string(), None);
        assert_eq!(client.base_url, "http://localhost:9090");
        assert!(client.token.is_none());
    }

    #[test]
    fn test_client_new_with_token() {
        let client = ZLayerClient::new(
            "http://localhost:9090/".to_string(),
            Some("test-token".to_string()),
        );
        // Trailing slash should be trimmed
        assert_eq!(client.base_url, "http://localhost:9090");
        assert_eq!(client.token, Some("test-token".to_string()));
    }

    #[test]
    fn test_client_set_token() {
        let mut client = ZLayerClient::new("http://localhost:9090".to_string(), None);
        assert!(client.token.is_none());

        client.set_token(Some("new-token".to_string()));
        assert_eq!(client.token, Some("new-token".to_string()));

        client.set_token(None);
        assert!(client.token.is_none());
    }

    #[test]
    fn test_health_response_deserialize() {
        // Without runtime_name -- should default to "auto"
        let json = r#"{"status":"ok","version":"0.1.0"}"#;
        let response: HealthResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.status, "ok");
        assert_eq!(response.version, "0.1.0");
        assert!(response.uptime_secs.is_none());
        assert_eq!(response.runtime_name, "auto");
    }

    #[test]
    fn test_health_response_deserialize_with_runtime() {
        let json = r#"{"status":"ok","version":"0.1.0","runtime_name":"docker"}"#;
        let response: HealthResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.runtime_name, "docker");
    }

    #[test]
    fn test_deployment_summary_deserialize() {
        let json = r#"{
            "name": "my-app",
            "status": "running",
            "service_count": 3,
            "created_at": "2025-01-22T00:00:00Z"
        }"#;
        let summary: DeploymentSummary = serde_json::from_str(json).unwrap();
        assert_eq!(summary.name, "my-app");
        assert_eq!(summary.status, "running");
        assert_eq!(summary.service_count, 3);
    }

    #[test]
    fn test_build_status_deserialize() {
        let json = r#"{
            "id": "test-123",
            "status": "complete",
            "image_id": "sha256:abc",
            "started_at": "2025-01-26T12:00:00Z",
            "completed_at": "2025-01-26T12:05:00Z"
        }"#;
        let status: BuildStatus = serde_json::from_str(json).unwrap();
        assert_eq!(status.id, "test-123");
        assert_eq!(status.status, BuildStateEnum::Complete);
        assert_eq!(status.image_id, Some("sha256:abc".to_string()));
        assert!(status.error.is_none());
    }

    #[test]
    fn test_build_state_enum_deserialize() {
        assert_eq!(
            serde_json::from_str::<BuildStateEnum>("\"pending\"").unwrap(),
            BuildStateEnum::Pending
        );
        assert_eq!(
            serde_json::from_str::<BuildStateEnum>("\"running\"").unwrap(),
            BuildStateEnum::Running
        );
        assert_eq!(
            serde_json::from_str::<BuildStateEnum>("\"complete\"").unwrap(),
            BuildStateEnum::Complete
        );
        assert_eq!(
            serde_json::from_str::<BuildStateEnum>("\"failed\"").unwrap(),
            BuildStateEnum::Failed
        );
    }

    #[test]
    fn test_service_summary_deserialize() {
        let json = r#"{
            "name": "api",
            "deployment": "my-app",
            "status": "running",
            "replicas": 3,
            "desired_replicas": 3
        }"#;
        let summary: ServiceSummary = serde_json::from_str(json).unwrap();
        assert_eq!(summary.name, "api");
        assert_eq!(summary.deployment, "my-app");
        assert_eq!(summary.replicas, 3);
    }

    // =========================================================================
    // Secrets Tests
    // =========================================================================

    #[test]
    fn test_secret_metadata_deserialize() {
        let json = r#"{
            "name": "api-key",
            "created_at": 1234567890,
            "updated_at": 1234567900,
            "version": 3
        }"#;
        let metadata: SecretMetadata = serde_json::from_str(json).unwrap();
        assert_eq!(metadata.name, "api-key");
        assert_eq!(metadata.created_at, 1_234_567_890);
        assert_eq!(metadata.updated_at, 1_234_567_900);
        assert_eq!(metadata.version, 3);
    }

    #[test]
    fn test_create_secret_request_serialize() {
        let request = CreateSecretRequest {
            name: "db-password".to_string(),
            value: "secret-value".to_string(),
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("db-password"));
        assert!(json.contains("secret-value"));
    }

    // =========================================================================
    // Jobs Tests
    // =========================================================================

    #[test]
    fn test_trigger_job_response_deserialize() {
        let json = r#"{
            "execution_id": "abc-123",
            "message": "Job triggered"
        }"#;
        let response: TriggerJobResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.execution_id, "abc-123");
        assert_eq!(response.message, "Job triggered");
    }

    #[test]
    fn test_job_execution_response_deserialize() {
        let json = r#"{
            "id": "exec-123",
            "job_name": "backup",
            "status": "completed",
            "started_at": "2025-01-25T12:00:00Z",
            "completed_at": "2025-01-25T12:01:00Z",
            "exit_code": 0,
            "logs": "Done!",
            "trigger": "cli",
            "duration_ms": 5000
        }"#;
        let response: JobExecutionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "exec-123");
        assert_eq!(response.job_name, "backup");
        assert_eq!(response.status, "completed");
        assert_eq!(response.exit_code, Some(0));
        assert_eq!(response.duration_ms, Some(5000));
    }

    #[test]
    fn test_job_execution_response_minimal() {
        let json = r#"{
            "id": "exec-456",
            "job_name": "sync",
            "status": "pending",
            "trigger": "endpoint"
        }"#;
        let response: JobExecutionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "exec-456");
        assert_eq!(response.status, "pending");
        assert!(response.started_at.is_none());
        assert!(response.completed_at.is_none());
        assert!(response.exit_code.is_none());
        assert!(response.logs.is_none());
        assert!(response.error.is_none());
        assert!(response.duration_ms.is_none());
    }

    // =========================================================================
    // Cron Tests
    // =========================================================================

    #[test]
    fn test_cron_job_response_deserialize() {
        let json = r#"{
            "name": "backup",
            "schedule": "0 0 * * * * *",
            "enabled": true,
            "last_run": "2025-01-25T00:00:00Z",
            "next_run": "2025-01-26T00:00:00Z"
        }"#;
        let response: CronJobResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.name, "backup");
        assert_eq!(response.schedule, "0 0 * * * * *");
        assert!(response.enabled);
        assert!(response.last_run.is_some());
        assert!(response.next_run.is_some());
    }

    #[test]
    fn test_cron_job_response_minimal() {
        let json = r#"{
            "name": "cleanup",
            "schedule": "0 * * * *",
            "enabled": false
        }"#;
        let response: CronJobResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.name, "cleanup");
        assert!(!response.enabled);
        assert!(response.last_run.is_none());
        assert!(response.next_run.is_none());
    }

    #[test]
    fn test_trigger_cron_response_deserialize() {
        let json = r#"{
            "execution_id": "cron-abc-123",
            "message": "Cron job triggered manually"
        }"#;
        let response: TriggerCronResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.execution_id, "cron-abc-123");
        assert!(response.message.contains("triggered"));
    }

    #[test]
    fn test_cron_status_response_deserialize() {
        let json = r#"{
            "name": "backup",
            "enabled": true,
            "message": "Cron job 'backup' enabled"
        }"#;
        let response: CronStatusResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.name, "backup");
        assert!(response.enabled);
        assert!(response.message.contains("enabled"));
    }

    // =========================================================================
    // Overlay Tests
    // =========================================================================

    #[test]
    fn test_overlay_status_response_deserialize() {
        let json = r#"{
            "interface": "wg0",
            "is_leader": true,
            "node_ip": "10.0.0.1",
            "cidr": "10.0.0.0/24",
            "port": 51820,
            "total_peers": 5,
            "healthy_peers": 4,
            "unhealthy_peers": 1,
            "last_check": 1706745600
        }"#;
        let response: OverlayStatusResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.interface, "wg0");
        assert!(response.is_leader);
        assert_eq!(response.node_ip, "10.0.0.1");
        assert_eq!(response.cidr, "10.0.0.0/24");
        assert_eq!(response.port, 51820);
        assert_eq!(response.total_peers, 5);
        assert_eq!(response.healthy_peers, 4);
        assert_eq!(response.unhealthy_peers, 1);
        assert_eq!(response.last_check, 1_706_745_600);
    }

    #[test]
    fn test_overlay_status_response_serialize() {
        let response = OverlayStatusResponse {
            interface: "wg0".to_string(),
            is_leader: false,
            node_ip: "10.0.0.2".to_string(),
            cidr: "10.0.0.0/24".to_string(),
            port: 51820,
            total_peers: 3,
            healthy_peers: 3,
            unhealthy_peers: 0,
            last_check: 1_706_745_600,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("wg0"));
        assert!(json.contains("10.0.0.2"));
        assert!(json.contains("\"is_leader\":false"));
    }

    #[test]
    fn test_overlay_peer_info_deserialize() {
        let json = r#"{
            "public_key": "abc123=",
            "overlay_ip": "10.0.0.5",
            "healthy": true,
            "last_handshake_secs": 30,
            "last_ping_ms": 5,
            "failure_count": 0,
            "last_check": 1706745600
        }"#;
        let peer: OverlayPeerInfo = serde_json::from_str(json).unwrap();
        assert_eq!(peer.public_key, "abc123=");
        assert_eq!(peer.overlay_ip, Some("10.0.0.5".to_string()));
        assert!(peer.healthy);
        assert_eq!(peer.last_handshake_secs, Some(30));
        assert_eq!(peer.last_ping_ms, Some(5));
        assert_eq!(peer.failure_count, 0);
    }

    #[test]
    fn test_overlay_peer_info_minimal() {
        let json = r#"{
            "public_key": "xyz789=",
            "healthy": false,
            "failure_count": 3,
            "last_check": 1706745600
        }"#;
        let peer: OverlayPeerInfo = serde_json::from_str(json).unwrap();
        assert_eq!(peer.public_key, "xyz789=");
        assert!(peer.overlay_ip.is_none());
        assert!(!peer.healthy);
        assert!(peer.last_handshake_secs.is_none());
        assert!(peer.last_ping_ms.is_none());
        assert_eq!(peer.failure_count, 3);
    }

    #[test]
    fn test_overlay_peer_info_serialize() {
        let peer = OverlayPeerInfo {
            public_key: "test-key=".to_string(),
            overlay_ip: Some("10.0.0.10".to_string()),
            healthy: true,
            last_handshake_secs: Some(15),
            last_ping_ms: Some(2),
            failure_count: 0,
            last_check: 1_706_745_600,
        };
        let json = serde_json::to_string(&peer).unwrap();
        assert!(json.contains("test-key="));
        assert!(json.contains("10.0.0.10"));
        assert!(json.contains("\"healthy\":true"));
    }

    #[test]
    fn test_peer_list_response_deserialize() {
        let json = r#"{
            "total": 2,
            "healthy": 1,
            "peers": [
                {
                    "public_key": "peer1=",
                    "overlay_ip": "10.0.0.2",
                    "healthy": true,
                    "last_handshake_secs": 10,
                    "last_ping_ms": 3,
                    "failure_count": 0,
                    "last_check": 1706745600
                },
                {
                    "public_key": "peer2=",
                    "healthy": false,
                    "failure_count": 5,
                    "last_check": 1706745600
                }
            ]
        }"#;
        let response: PeerListResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.total, 2);
        assert_eq!(response.healthy, 1);
        assert_eq!(response.peers.len(), 2);
        assert!(response.peers[0].healthy);
        assert!(!response.peers[1].healthy);
    }

    #[test]
    fn test_peer_list_response_empty() {
        let json = r#"{
            "total": 0,
            "healthy": 0,
            "peers": []
        }"#;
        let response: PeerListResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.total, 0);
        assert_eq!(response.healthy, 0);
        assert!(response.peers.is_empty());
    }

    #[test]
    fn test_ip_allocation_response_deserialize() {
        let json = r#"{
            "cidr": "10.0.0.0/24",
            "total_ips": 254,
            "allocated_count": 10,
            "available_count": 244,
            "utilization_percent": 3.94
        }"#;
        let response: IpAllocationResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.cidr, "10.0.0.0/24");
        assert_eq!(response.total_ips, 254);
        assert_eq!(response.allocated_count, 10);
        assert_eq!(response.available_count, 244);
        assert!((response.utilization_percent - 3.94).abs() < 0.01);
    }

    #[test]
    fn test_ip_allocation_response_serialize() {
        let response = IpAllocationResponse {
            cidr: "192.168.0.0/16".to_string(),
            total_ips: 65534,
            allocated_count: 100,
            available_count: 65434,
            utilization_percent: 0.15,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("192.168.0.0/16"));
        assert!(json.contains("65534"));
        assert!(json.contains("\"allocated_count\":100"));
    }

    #[test]
    fn test_dns_status_response_deserialize() {
        let json = r#"{
            "enabled": true,
            "zone": "zlayer.local",
            "port": 53,
            "bind_addr": "10.0.0.1",
            "service_count": 3,
            "services": ["api", "web", "db"]
        }"#;
        let response: DnsStatusResponse = serde_json::from_str(json).unwrap();
        assert!(response.enabled);
        assert_eq!(response.zone, Some("zlayer.local".to_string()));
        assert_eq!(response.port, Some(53));
        assert_eq!(response.bind_addr, Some("10.0.0.1".to_string()));
        assert_eq!(response.service_count, 3);
        assert_eq!(response.services.len(), 3);
        assert!(response.services.contains(&"api".to_string()));
    }

    #[test]
    fn test_dns_status_response_disabled() {
        let json = r#"{
            "enabled": false,
            "service_count": 0,
            "services": []
        }"#;
        let response: DnsStatusResponse = serde_json::from_str(json).unwrap();
        assert!(!response.enabled);
        assert!(response.zone.is_none());
        assert!(response.port.is_none());
        assert!(response.bind_addr.is_none());
        assert_eq!(response.service_count, 0);
        assert!(response.services.is_empty());
    }

    #[test]
    fn test_dns_status_response_serialize() {
        let response = DnsStatusResponse {
            enabled: true,
            zone: Some("test.local".to_string()),
            port: Some(5353),
            bind_addr: Some("0.0.0.0".to_string()),
            service_count: 2,
            services: vec!["svc1".to_string(), "svc2".to_string()],
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("test.local"));
        assert!(json.contains("5353"));
        assert!(json.contains("svc1"));
        assert!(json.contains("svc2"));
    }

    // =========================================================================
    // API Client Error Tests
    // =========================================================================

    #[test]
    fn test_api_client_error_display() {
        let http_err = ApiClientError::Api {
            status: 500,
            message: "Internal Server Error".to_string(),
        };
        assert!(http_err.to_string().contains("500"));
        assert!(http_err.to_string().contains("Internal Server Error"));

        let not_found = ApiClientError::NotFound("Resource xyz".to_string());
        assert!(not_found.to_string().contains("xyz"));

        let unauthorized = ApiClientError::Unauthorized("Invalid token".to_string());
        assert!(unauthorized.to_string().contains("Invalid token"));

        let rate_limited = ApiClientError::RateLimited;
        assert!(rate_limited.to_string().contains("Rate limited"));

        let deserialize = ApiClientError::Deserialize("invalid json".to_string());
        assert!(deserialize.to_string().contains("invalid json"));
    }

    // =========================================================================
    // Round-trip serialization tests
    // =========================================================================

    #[test]
    fn test_overlay_status_roundtrip() {
        let original = OverlayStatusResponse {
            interface: "wg0".to_string(),
            is_leader: true,
            node_ip: "10.0.0.1".to_string(),
            cidr: "10.0.0.0/24".to_string(),
            port: 51820,
            total_peers: 5,
            healthy_peers: 4,
            unhealthy_peers: 1,
            last_check: 1_706_745_600,
        };
        let json = serde_json::to_string(&original).unwrap();
        let restored: OverlayStatusResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(original.interface, restored.interface);
        assert_eq!(original.is_leader, restored.is_leader);
        assert_eq!(original.node_ip, restored.node_ip);
        assert_eq!(original.total_peers, restored.total_peers);
    }

    #[test]
    fn test_ip_allocation_roundtrip() {
        let original = IpAllocationResponse {
            cidr: "10.0.0.0/24".to_string(),
            total_ips: 254,
            allocated_count: 50,
            available_count: 204,
            utilization_percent: 19.69,
        };
        let json = serde_json::to_string(&original).unwrap();
        let restored: IpAllocationResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(original.cidr, restored.cidr);
        assert_eq!(original.total_ips, restored.total_ips);
        assert_eq!(original.allocated_count, restored.allocated_count);
    }

    #[test]
    fn test_dns_status_roundtrip() {
        let original = DnsStatusResponse {
            enabled: true,
            zone: Some("zlayer.local".to_string()),
            port: Some(53),
            bind_addr: Some("10.0.0.1".to_string()),
            service_count: 5,
            services: vec!["a".to_string(), "b".to_string(), "c".to_string()],
        };
        let json = serde_json::to_string(&original).unwrap();
        let restored: DnsStatusResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(original.enabled, restored.enabled);
        assert_eq!(original.zone, restored.zone);
        assert_eq!(original.services, restored.services);
    }

    mod raw_tests {
        use super::super::{RawMethod, RawResponse};
        use reqwest::StatusCode;

        #[test]
        fn raw_method_round_trip() {
            // Each variant maps consistently across both transport conversions.
            for m in [
                RawMethod::Get,
                RawMethod::Post,
                RawMethod::Patch,
                RawMethod::Delete,
            ] {
                assert_eq!(m.as_reqwest().as_str(), format!("{m:?}").to_uppercase());
            }
        }

        #[test]
        fn raw_response_fields_are_public() {
            let r = RawResponse {
                status: StatusCode::OK,
                body: b"{}".to_vec(),
                set_cookies: vec!["a=b".into()],
            };
            assert_eq!(r.status, StatusCode::OK);
            assert_eq!(r.body, b"{}".to_vec());
            assert_eq!(r.set_cookies.len(), 1);
        }
    }
}
