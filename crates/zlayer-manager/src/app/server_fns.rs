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

/// Get a ZLayer API client configured from the environment.
///
/// Preferred transport:
/// 1. Unix Domain Socket — if `ZLAYER_SOCKET` points at an existing file, use
///    `ZLayerClient::new_unix`. Works inside overlay-networked containers
///    where TCP loopback to the host is unreachable.
/// 2. TCP — otherwise use `ZLAYER_API_URL` (defaulting to
///    `DEFAULT_API_URL`).
///
/// In both modes `ZLAYER_API_TOKEN` (if set) becomes the Bearer token.
#[cfg(feature = "ssr")]
fn get_api_client() -> ZLayerClient {
    let token = std::env::var("ZLAYER_API_TOKEN").ok();

    if let Ok(socket) = std::env::var("ZLAYER_SOCKET") {
        let path = std::path::PathBuf::from(&socket);
        if path.exists() {
            return ZLayerClient::new_unix(path, token);
        }
        tracing::warn!(
            socket = %socket,
            "ZLAYER_SOCKET is set but the socket file does not exist; falling back to TCP"
        );
    }

    let base_url = std::env::var("ZLAYER_API_URL").unwrap_or_else(|_| DEFAULT_API_URL.to_string());
    ZLayerClient::new(base_url, token)
}

/// Convert `ApiClientError` to `ServerFnError`
#[cfg(feature = "ssr")]
fn api_error_to_server_error(err: &ApiClientError) -> ServerFnError {
    ServerFnError::new(err.to_string())
}

/// Headers forwarded from the browser's request to the upstream API — the
/// session cookie and the double-submit CSRF token.
#[cfg(feature = "ssr")]
struct ForwardedHeaders {
    /// Raw `Cookie:` header value, if the browser sent one.
    cookie: Option<String>,
    /// `x-csrf-token` header value, if the browser sent one.
    csrf: Option<String>,
}

/// Extract the `Cookie` and `x-csrf-token` headers from the current axum
/// request so we can forward them to the zlayer-api backend.
///
/// Returns an empty `ForwardedHeaders` if the request extractor fails (e.g.
/// no axum request is in scope — shouldn't happen inside a server_fn, but
/// we stay defensive rather than panicking).
#[cfg(feature = "ssr")]
async fn extract_forwarded_headers() -> ForwardedHeaders {
    use axum::http::HeaderMap;
    let Ok(headers) = leptos_axum::extract::<HeaderMap>().await else {
        return ForwardedHeaders {
            cookie: None,
            csrf: None,
        };
    };
    let cookie = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let csrf = headers
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    ForwardedHeaders { cookie, csrf }
}

/// Append each upstream `Set-Cookie` header onto the Leptos response. Uses
/// `append_header` (not `insert_header`) so multiple `Set-Cookie` values
/// coexist — the typical login flow emits both a session cookie and a
/// CSRF-token cookie in one response.
#[cfg(feature = "ssr")]
fn propagate_set_cookies(cookies: &[String]) {
    use axum::http::{header::SET_COOKIE, HeaderValue};
    let Some(opts) = use_context::<leptos_axum::ResponseOptions>() else {
        tracing::warn!("ResponseOptions not in context; cannot propagate Set-Cookie headers");
        return;
    };
    for c in cookies {
        match HeaderValue::from_str(c) {
            Ok(v) => opts.append_header(SET_COOKIE, v),
            Err(e) => tracing::warn!(error = %e, cookie = %c, "invalid Set-Cookie value; skipping"),
        }
    }
}

/// Pull a human-readable error message out of an upstream JSON error body.
/// Falls back to the raw body when the payload isn't JSON with a `message`
/// or `error` string field.
#[cfg(feature = "ssr")]
fn parse_error_message(body: &[u8]) -> String {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("message")
                .or_else(|| v.get("error"))
                .and_then(|s| s.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| String::from_utf8_lossy(body).into_owned())
}

/// Percent-encode a path segment. UUIDs and simple ids never need encoding,
/// but we stay safe against caller-supplied input that might contain `/`,
/// spaces, or other reserved characters.
#[cfg(feature = "ssr")]
fn pct_encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(&mut out, "%{b:02X}");
        }
    }
    out
}

// ============================================================================
// Response Types
// ============================================================================

/// Default runtime name when the field is absent from JSON (backwards compat).
fn default_runtime_name() -> String {
    "auto".to_string()
}

// ============================================================================
// Auth / users types (local mirrors of zlayer-api types for client-side use)
// ============================================================================

/// A user record as returned by the auth/users API, re-shaped for the
/// hydrate-side code (no dependency on `zlayer-api`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagerUserView {
    /// Stable user identifier (UUID, opaque).
    pub id: String,
    /// Email address (login identifier).
    pub email: String,
    /// Display name.
    pub display_name: String,
    /// Role — "admin" or "user".
    pub role: String,
    /// Whether the account is active (disabled accounts cannot log in).
    pub is_active: bool,
    /// Last successful login as RFC-3339 string, or `None` if the user has
    /// never logged in.
    pub last_login_at: Option<String>,
}

/// Credentials submitted by the login form.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagerLoginRequest {
    /// Email address.
    pub email: String,
    /// Plaintext password (TLS-terminated in production).
    pub password: String,
}

/// Response returned by a successful login or bootstrap.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagerLoginResponse {
    /// The user that just authenticated.
    pub user: ManagerUserView,
    /// CSRF token to be stored in memory / sent back via `x-csrf-token`.
    pub csrf_token: String,
}

/// First-run admin-creation payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagerBootstrapRequest {
    /// Email address for the new admin.
    pub email: String,
    /// Plaintext password for the new admin.
    pub password: String,
    /// Optional display name — backend falls back to the email's local part.
    pub display_name: Option<String>,
}

/// Admin-initiated user creation payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagerCreateUserRequest {
    /// Email address.
    pub email: String,
    /// Initial password.
    pub password: String,
    /// Optional display name.
    pub display_name: Option<String>,
    /// "admin" or "user".
    pub role: String,
}

/// Patch-style update payload — every field is optional.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagerUpdateUserRequest {
    /// New display name, or leave unchanged.
    pub display_name: Option<String>,
    /// New role ("admin" or "user"), or leave unchanged.
    pub role: Option<String>,
    /// New active flag, or leave unchanged.
    pub is_active: Option<bool>,
}

/// Admin-initiated password reset payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagerSetPasswordRequest {
    /// New password.
    pub new_password: String,
    /// Required when a non-admin resets their own password; optional
    /// otherwise (admin bypass).
    pub current_password: Option<String>,
}

/// Aggregated response for `/auth/me` — tells the UI whether to render the
/// login form, the bootstrap form, or an authenticated view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagerMeResponse {
    /// Some(user) when signed in; None when anonymous.
    pub user: Option<ManagerUserView>,
    /// True iff the backend has no users at all (first-run).
    pub needs_bootstrap: bool,
}

/// System statistics for the manager dashboard
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SystemStats {
    /// Version of zlayer-manager
    pub version: String,
    /// Container runtime name (e.g. "youki", "mac-sandbox", "docker")
    #[serde(default = "default_runtime_name")]
    pub runtime_name: String,
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

/// Node information from the Raft cluster state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    /// Raft-level node identifier
    pub id: String,
    /// Advertise address (public IP)
    pub address: String,
    /// Node status (e.g., "ready", "draining", "dead")
    pub status: String,
    /// Role in the Raft cluster: "leader", "voter", or "learner"
    pub role: String,
    /// Join mode: "full" or "replicate"
    pub mode: String,
    /// Whether this node is the Raft leader
    pub is_leader: bool,
    /// Overlay network IP
    pub overlay_ip: String,
    /// Total CPU cores
    pub cpu_total: f64,
    /// Used CPU cores
    pub cpu_used: f64,
    /// Total memory in bytes
    pub memory_total: u64,
    /// Used memory in bytes
    pub memory_used: u64,
    /// When the node was registered (Unix timestamp ms)
    pub registered_at: u64,
    /// Last heartbeat timestamp (Unix timestamp ms)
    pub last_heartbeat: u64,
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

/// Service endpoint information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceEndpoint {
    /// Endpoint name
    pub name: String,
    /// Protocol (http, https, tcp, websocket, etc.)
    pub protocol: String,
    /// Port number
    pub port: u16,
    /// URL (if publicly exposed)
    pub url: Option<String>,
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
    /// Service endpoints (ports, protocols, URLs)
    pub endpoints: Vec<ServiceEndpoint>,
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

    // Get cluster nodes for node counts and aggregate resource usage
    let cluster_nodes = client.list_cluster_nodes().await.unwrap_or_default();

    // SAFETY: Number of deployments will never exceed u32::MAX in practice
    #[allow(clippy::cast_possible_truncation)]
    let total_deployments = deployments.len() as u32;
    // SAFETY: Number of active deployments will never exceed u32::MAX in practice
    #[allow(clippy::cast_possible_truncation)]
    let active_deployments = deployments.iter().filter(|d| d.status == "running").count() as u32;

    // SAFETY: Number of nodes will never exceed u32::MAX in practice
    #[allow(clippy::cast_possible_truncation)]
    let total_nodes = cluster_nodes.len() as u32;
    // SAFETY: Number of healthy nodes will never exceed u32::MAX in practice
    #[allow(clippy::cast_possible_truncation)]
    let healthy_nodes = cluster_nodes.iter().filter(|n| n.status == "ready").count() as u32;

    // Aggregate CPU and memory across cluster
    let total_cpu: f64 = cluster_nodes.iter().map(|n| n.cpu_total).sum();
    let used_cpu: f64 = cluster_nodes.iter().map(|n| n.cpu_used).sum();
    let total_mem: u64 = cluster_nodes.iter().map(|n| n.memory_total).sum();
    let used_mem: u64 = cluster_nodes.iter().map(|n| n.memory_used).sum();

    let cpu_percent = if total_cpu > 0.0 {
        (used_cpu / total_cpu) * 100.0
    } else {
        0.0
    };
    let memory_percent = if total_mem > 0 {
        #[allow(clippy::cast_precision_loss)]
        let pct = (used_mem as f64 / total_mem as f64) * 100.0;
        pct
    } else {
        0.0
    };

    Ok(SystemStats {
        version: health.version,
        runtime_name: health.runtime_name,
        total_nodes,
        healthy_nodes,
        total_deployments,
        active_deployments,
        cpu_percent,
        memory_percent,
        uptime_seconds: health.uptime_secs.unwrap_or(0),
    })
}

/// Get the runtime name from the API health endpoint.
///
/// Lightweight call used by the navbar badge so it doesn't need to fetch
/// full system stats.
#[server(prefix = "/api/manager")]
pub async fn get_runtime_name() -> Result<String, ServerFnError> {
    let client = get_api_client();
    let health = client
        .health()
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    Ok(health.runtime_name)
}

/// Get all nodes in the cluster
#[server(prefix = "/api/manager")]
pub async fn get_nodes() -> Result<Vec<Node>, ServerFnError> {
    let client = get_api_client();

    let cluster_nodes = client
        .list_cluster_nodes()
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    Ok(cluster_nodes
        .into_iter()
        .map(|n| Node {
            id: n.id,
            address: n.advertise_addr,
            status: n.status,
            role: n.role,
            mode: n.mode,
            is_leader: n.is_leader,
            overlay_ip: n.overlay_ip,
            cpu_total: n.cpu_total,
            cpu_used: n.cpu_used,
            memory_total: n.memory_total,
            memory_used: n.memory_used,
            registered_at: n.registered_at,
            last_heartbeat: n.last_heartbeat,
        })
        .collect())
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

/// Create a new deployment from YAML spec
#[server(CreateDeployment, prefix = "/api/manager")]
pub async fn create_deployment(yaml: String) -> Result<(), ServerFnError> {
    let client = get_api_client();
    client
        .create_deployment(&yaml)
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    Ok(())
}

/// Delete a deployment by name
#[server(DeleteDeployment, prefix = "/api/manager")]
pub async fn delete_deployment(name: String) -> Result<(), ServerFnError> {
    let client = get_api_client();
    client
        .delete_deployment(&name)
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    Ok(())
}

/// Get services for a specific deployment
#[server(GetServices, prefix = "/api/manager")]
pub async fn get_services(deployment: String) -> Result<Vec<Service>, ServerFnError> {
    let client = get_api_client();

    let summaries = client
        .list_services(&deployment)
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    // Fetch details for each service to get endpoint information
    let mut services = Vec::with_capacity(summaries.len());
    for summary in &summaries {
        let endpoints = match client.get_service(&deployment, &summary.name).await {
            Ok(details) => details
                .endpoints
                .into_iter()
                .map(|ep| ServiceEndpoint {
                    name: ep.name,
                    protocol: ep.protocol,
                    port: ep.port,
                    url: ep.url,
                })
                .collect(),
            Err(_) => Vec::new(),
        };

        services.push(Service {
            name: summary.name.clone(),
            deployment: summary.deployment.clone(),
            status: summary.status.clone(),
            replicas: summary.replicas,
            desired_replicas: summary.desired_replicas,
            endpoints,
        });
    }

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

/// Trigger a new build
#[server(TriggerBuild, prefix = "/api/manager")]
pub async fn trigger_build(
    context_path: String,
    tags: String,
    runtime: String,
) -> Result<String, ServerFnError> {
    let client = get_api_client();

    let tag_list: Vec<String> = tags
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let runtime_opt = if runtime.trim().is_empty() {
        None
    } else {
        Some(runtime.trim().to_string())
    };

    let request = crate::api_client::TriggerBuildRequest {
        context_path,
        tags: tag_list,
        runtime: runtime_opt,
        no_cache: false,
    };

    let response = client
        .trigger_build(&request)
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    Ok(response.build_id)
}

// ============================================================================
// Tunnel Server Functions
// ============================================================================

/// Tunnel information for UI display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelInfo {
    /// Unique tunnel identifier
    pub id: String,
    /// Name of the tunnel
    pub name: String,
    /// Current status
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

/// List all tunnels
#[server(GetTunnels, prefix = "/api/manager")]
pub async fn get_tunnels() -> Result<Vec<TunnelInfo>, ServerFnError> {
    let client = get_api_client();
    let tunnels = client
        .list_tunnels()
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    Ok(tunnels
        .into_iter()
        .map(|t| TunnelInfo {
            id: t.id,
            name: t.name,
            status: t.status,
            services: t.services,
            created_at: t.created_at,
            expires_at: t.expires_at,
            last_connected: t.last_connected,
        })
        .collect())
}

/// Create a new tunnel
#[server(CreateTunnel, prefix = "/api/manager")]
pub async fn create_tunnel(
    name: String,
    services: String,
    ttl_hours: u64,
) -> Result<TunnelInfo, ServerFnError> {
    let client = get_api_client();

    let service_list: Vec<String> = services
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let request = crate::api_client::CreateTunnelRequest {
        name,
        services: service_list,
        ttl_secs: ttl_hours * 3600,
    };

    let response = client
        .create_tunnel(&request)
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    Ok(TunnelInfo {
        id: response.id,
        name: response.name,
        status: "pending".to_string(),
        services: response.services,
        created_at: response.created_at,
        expires_at: response.expires_at,
        last_connected: None,
    })
}

/// Delete a tunnel
#[server(DeleteTunnel, prefix = "/api/manager")]
pub async fn delete_tunnel(id: String) -> Result<(), ServerFnError> {
    let client = get_api_client();
    client
        .delete_tunnel(&id)
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    Ok(())
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
// Cluster Reset Server Function
// ============================================================================

/// Reset the entire cluster by deleting all deployments and secrets
#[server(ResetCluster, prefix = "/api/manager")]
pub async fn reset_cluster() -> Result<String, ServerFnError> {
    let client = get_api_client();

    let mut deleted_deployments = 0u32;
    let mut deleted_secrets = 0u32;
    let mut errors = Vec::new();

    // Delete all deployments
    let deployments = client
        .list_deployments()
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    for dep in &deployments {
        if let Err(e) = client.delete_deployment(&dep.name).await {
            errors.push(format!("deployment '{}': {}", dep.name, e));
        } else {
            deleted_deployments += 1;
        }
    }

    // Delete all secrets
    let secrets = client
        .list_secrets()
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    for secret in &secrets {
        if let Err(e) = client.delete_secret(&secret.name).await {
            errors.push(format!("secret '{}': {}", secret.name, e));
        } else {
            deleted_secrets += 1;
        }
    }

    if errors.is_empty() {
        Ok(format!(
            "Cluster reset complete. Deleted {deleted_deployments} deployment(s) and {deleted_secrets} secret(s).",
        ))
    } else {
        Err(ServerFnError::new(format!(
            "Partial reset: deleted {deleted_deployments} deployment(s) and {deleted_secrets} secret(s), but {} error(s) occurred: {}",
            errors.len(),
            errors.join("; ")
        )))
    }
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
// Proxy Types
// ============================================================================

/// Backend summary for proxy UI
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyBackendInfo {
    /// Backend address (host:port)
    pub address: String,
    /// Whether the backend is healthy
    pub healthy: bool,
    /// Number of active connections
    pub active_connections: u64,
}

/// Proxy route information for UI
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyRouteInfo {
    /// Host to match (e.g., "api.example.com")
    pub host: Option<String>,
    /// Path prefix to match (e.g., "/api")
    pub path_prefix: String,
    /// Whether to strip the matched prefix before forwarding
    pub strip_prefix: bool,
    /// Backends serving this route
    pub backends: Vec<ProxyBackendInfo>,
}

/// Proxy backend group information for UI
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyBackendGroupInfo {
    /// Service name
    pub service: String,
    /// Load-balancing strategy ("round_robin" or "least_connections")
    pub strategy: String,
    /// Backends in this group
    pub backends: Vec<ProxyBackendInfo>,
}

/// TLS certificate information for UI
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsCertificateInfo {
    /// Domain the certificate covers
    pub domain: String,
    /// Certificate issuer (e.g., "Let's Encrypt")
    pub issuer: Option<String>,
    /// When the certificate expires (ISO 8601)
    pub expires_at: Option<String>,
    /// Whether auto-renewal is enabled
    pub auto_renew: bool,
}

/// Stream proxy information for UI
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamProxyInfo {
    /// Protocol ("tcp" or "udp")
    pub protocol: String,
    /// Port the proxy listens on
    pub listen_port: u16,
    /// Backends receiving forwarded traffic
    pub backends: Vec<ProxyBackendInfo>,
}

// ============================================================================
// Proxy Server Functions
// ============================================================================

/// List all proxy routes
#[server(GetProxyRoutes, prefix = "/api/manager")]
pub async fn get_proxy_routes() -> Result<Vec<ProxyRouteInfo>, ServerFnError> {
    let client = get_api_client();
    let routes = client
        .list_proxy_routes()
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    Ok(routes
        .into_iter()
        .map(|r| ProxyRouteInfo {
            host: r.host,
            path_prefix: r.path_prefix,
            strip_prefix: r.strip_prefix,
            backends: r
                .backends
                .into_iter()
                .map(|b| ProxyBackendInfo {
                    address: b.address,
                    healthy: b.healthy,
                    active_connections: b.active_connections,
                })
                .collect(),
        })
        .collect())
}

/// List all proxy backend groups
#[server(GetProxyBackends, prefix = "/api/manager")]
pub async fn get_proxy_backends() -> Result<Vec<ProxyBackendGroupInfo>, ServerFnError> {
    let client = get_api_client();
    let groups = client
        .list_proxy_backends()
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    Ok(groups
        .into_iter()
        .map(|g| ProxyBackendGroupInfo {
            service: g.service,
            strategy: g.strategy,
            backends: g
                .backends
                .into_iter()
                .map(|b| ProxyBackendInfo {
                    address: b.address,
                    healthy: b.healthy,
                    active_connections: b.active_connections,
                })
                .collect(),
        })
        .collect())
}

/// List all TLS certificates
#[server(GetTlsCertificates, prefix = "/api/manager")]
pub async fn get_tls_certificates() -> Result<Vec<TlsCertificateInfo>, ServerFnError> {
    let client = get_api_client();
    let certs = client
        .list_tls_certificates()
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    Ok(certs
        .into_iter()
        .map(|c| TlsCertificateInfo {
            domain: c.domain,
            issuer: c.issuer,
            expires_at: c.expires_at,
            auto_renew: c.auto_renew,
        })
        .collect())
}

/// List all stream proxies
#[server(GetStreamProxies, prefix = "/api/manager")]
pub async fn get_stream_proxies() -> Result<Vec<StreamProxyInfo>, ServerFnError> {
    let client = get_api_client();
    let streams = client
        .list_stream_proxies()
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    Ok(streams
        .into_iter()
        .map(|s| StreamProxyInfo {
            protocol: s.protocol,
            listen_port: s.listen_port,
            backends: s
                .backends
                .into_iter()
                .map(|b| ProxyBackendInfo {
                    address: b.address,
                    healthy: b.healthy,
                    active_connections: b.active_connections,
                })
                .collect(),
        })
        .collect())
}

// ============================================================================
// Network Server Functions
// ============================================================================

/// Network summary for UI display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkInfo {
    /// Network name
    pub name: String,
    /// Optional description
    pub description: Option<String>,
    /// Number of CIDR ranges
    pub cidr_count: usize,
    /// Number of members
    pub member_count: usize,
    /// Number of access rules
    pub rule_count: usize,
}

/// Full network detail for the detail modal
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkDetail {
    /// Network name
    pub name: String,
    /// Optional description
    pub description: Option<String>,
    /// CIDR ranges
    pub cidrs: Vec<String>,
    /// Members
    pub members: Vec<NetworkMemberItem>,
    /// Access rules
    pub access_rules: Vec<NetworkAccessRuleItem>,
}

/// A member of a network (UI type)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkMemberItem {
    /// Member identifier
    pub name: String,
    /// Type of member (user, group, node, cidr)
    pub kind: String,
}

/// An access rule within a network (UI type)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkAccessRuleItem {
    /// Target service name, or "*" for all
    pub service: String,
    /// Target deployment name, or "*" for all
    pub deployment: String,
    /// Specific ports, or empty for all
    pub ports: String,
    /// Allow or Deny
    pub action: String,
}

/// List all networks
#[server(GetNetworks, prefix = "/api/manager")]
pub async fn get_networks() -> Result<Vec<NetworkInfo>, ServerFnError> {
    let client = get_api_client();
    let networks = client
        .list_networks()
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    Ok(networks
        .into_iter()
        .map(|n| NetworkInfo {
            name: n.name,
            description: n.description,
            cidr_count: n.cidr_count,
            member_count: n.member_count,
            rule_count: n.rule_count,
        })
        .collect())
}

/// Get a specific network by name
#[server(GetNetwork, prefix = "/api/manager")]
pub async fn get_network(name: String) -> Result<NetworkDetail, ServerFnError> {
    let client = get_api_client();
    let detail = client
        .get_network(&name)
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    Ok(NetworkDetail {
        name: detail.name,
        description: detail.description,
        cidrs: detail.cidrs,
        members: detail
            .members
            .into_iter()
            .map(|m| NetworkMemberItem {
                name: m.name,
                kind: m.kind,
            })
            .collect(),
        access_rules: detail
            .access_rules
            .into_iter()
            .map(|r| NetworkAccessRuleItem {
                service: r.service,
                deployment: r.deployment,
                ports: r.ports.map_or_else(
                    || "all".to_string(),
                    |p| {
                        p.iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    },
                ),
                action: r.action,
            })
            .collect(),
    })
}

/// Create a new network
#[server(CreateNetwork, prefix = "/api/manager")]
pub async fn create_network(
    name: String,
    description: String,
    cidrs: String,
) -> Result<NetworkInfo, ServerFnError> {
    let client = get_api_client();

    let cidr_list: Vec<String> = cidrs
        .split([',', '\n'])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let request = crate::api_client::CreateNetworkRequest {
        name: name.clone(),
        description: if description.trim().is_empty() {
            None
        } else {
            Some(description)
        },
        cidrs: cidr_list.clone(),
        members: Vec::new(),
        access_rules: Vec::new(),
    };

    let response = client
        .create_network(&request)
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    Ok(NetworkInfo {
        name: response.name,
        description: response.description,
        cidr_count: response.cidrs.len(),
        member_count: response.members.len(),
        rule_count: response.access_rules.len(),
    })
}

/// Delete a network
#[server(DeleteNetwork, prefix = "/api/manager")]
pub async fn delete_network(name: String) -> Result<(), ServerFnError> {
    let client = get_api_client();
    client
        .delete_network(&name)
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    Ok(())
}

// ============================================================================
// Auth + users (proxy to zlayer-api with cookie / CSRF forwarding)
// ============================================================================

/// POST `/auth/login` — log the user in. Returns the user + csrf token and
/// sets the `zlayer_session` cookie on the browser.
#[server(prefix = "/api/manager")]
pub async fn manager_login(
    req: ManagerLoginRequest,
) -> Result<ManagerLoginResponse, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let body =
        serde_json::to_vec(&req).map_err(|e| ServerFnError::new(format!("serialise: {e}")))?;

    let resp = client
        .raw_request(
            RawMethod::Post,
            "/auth/login",
            Some(&body),
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    propagate_set_cookies(&resp.set_cookies);

    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Login failed ({}): {msg}",
            resp.status
        )));
    }

    serde_json::from_slice(&resp.body).map_err(|e| ServerFnError::new(format!("deserialise: {e}")))
}

/// POST `/auth/bootstrap` — create the first admin user. Only succeeds when
/// the user table is empty; otherwise the backend returns 409.
#[server(prefix = "/api/manager")]
pub async fn manager_bootstrap(
    req: ManagerBootstrapRequest,
) -> Result<ManagerLoginResponse, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let body =
        serde_json::to_vec(&req).map_err(|e| ServerFnError::new(format!("serialise: {e}")))?;

    let resp = client
        .raw_request(
            RawMethod::Post,
            "/auth/bootstrap",
            Some(&body),
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    propagate_set_cookies(&resp.set_cookies);

    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Bootstrap failed ({}): {msg}",
            resp.status
        )));
    }

    serde_json::from_slice(&resp.body).map_err(|e| ServerFnError::new(format!("deserialise: {e}")))
}

/// POST `/auth/logout` — clear the session cookie. Always forwards any
/// `Set-Cookie` headers from the backend (which typically include an
/// expired-cookie header to clear the browser state).
#[server(prefix = "/api/manager")]
pub async fn manager_logout() -> Result<(), ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let resp = client
        .raw_request(
            RawMethod::Post,
            "/auth/logout",
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    propagate_set_cookies(&resp.set_cookies);

    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Logout failed ({}): {msg}",
            resp.status
        )));
    }
    Ok(())
}

/// GET `/auth/me` — returns the current user, or `needs_bootstrap: true`
/// when no users exist on the backend.
///
/// Never returns an error for the "not authenticated" case — it returns
/// `Ok(ManagerMeResponse { user: None, .. })`. `/auth/me` alone doesn't
/// tell us whether the store is empty (it returns 401 either way), so on
/// 401 we probe `/auth/bootstrap` with an empty body:
///   - 400 → empty creds rejected, meaning the user table is empty and
///     bootstrap is available (`needs_bootstrap = true`).
///   - 409 → bootstrap already completed, at least one user exists
///     (`needs_bootstrap = false`).
///   - anything else → conservative `needs_bootstrap = false`.
#[server(prefix = "/api/manager")]
pub async fn manager_me() -> Result<ManagerMeResponse, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();

    let me_resp = client
        .raw_request(
            RawMethod::Get,
            "/auth/me",
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    if me_resp.status.is_success() {
        let user: ManagerUserView = serde_json::from_slice(&me_resp.body)
            .map_err(|e| ServerFnError::new(format!("deserialise /auth/me: {e}")))?;
        return Ok(ManagerMeResponse {
            user: Some(user),
            needs_bootstrap: false,
        });
    }

    // Not authenticated — probe bootstrap state with an intentionally
    // invalid (empty) body. We don't forward cookies/csrf here — this is
    // a pure state probe against the unauthenticated endpoint.
    let probe_body = b"{\"email\":\"\",\"password\":\"\"}";
    let probe = client
        .raw_request(
            RawMethod::Post,
            "/auth/bootstrap",
            Some(probe_body),
            None,
            None,
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;

    // 400 means the backend parsed our body but rejected the empty creds —
    // which it only does when the user table is empty (and bootstrap is
    // therefore available). 409 means bootstrap is already done; any other
    // status we treat conservatively as "bootstrap not available" to avoid
    // offering an endpoint that would fail.
    let needs_bootstrap = probe.status.as_u16() == 400;

    Ok(ManagerMeResponse {
        user: None,
        needs_bootstrap,
    })
}

/// GET `/api/v1/users` — list all users. Admin only; the backend enforces.
#[server(prefix = "/api/manager")]
pub async fn manager_list_users() -> Result<Vec<ManagerUserView>, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let resp = client
        .raw_request(
            RawMethod::Get,
            "/api/v1/users",
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "List users failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise users: {e}")))
}

/// POST `/api/v1/users` — create a new user. Admin only.
#[server(prefix = "/api/manager")]
pub async fn manager_create_user(
    req: ManagerCreateUserRequest,
) -> Result<ManagerUserView, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let body =
        serde_json::to_vec(&req).map_err(|e| ServerFnError::new(format!("serialise: {e}")))?;
    let resp = client
        .raw_request(
            RawMethod::Post,
            "/api/v1/users",
            Some(&body),
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Create user failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body).map_err(|e| ServerFnError::new(format!("deserialise: {e}")))
}

/// PATCH `/api/v1/users/{id}` — update a user's display name, role, or
/// active flag. Admin only.
#[server(prefix = "/api/manager")]
pub async fn manager_update_user(
    id: String,
    req: ManagerUpdateUserRequest,
) -> Result<ManagerUserView, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let body =
        serde_json::to_vec(&req).map_err(|e| ServerFnError::new(format!("serialise: {e}")))?;
    let path = format!("/api/v1/users/{}", pct_encode_path(&id));
    let resp = client
        .raw_request(
            RawMethod::Patch,
            &path,
            Some(&body),
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Update user failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body).map_err(|e| ServerFnError::new(format!("deserialise: {e}")))
}

/// DELETE `/api/v1/users/{id}`.
#[server(prefix = "/api/manager")]
pub async fn manager_delete_user(id: String) -> Result<(), ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = format!("/api/v1/users/{}", pct_encode_path(&id));
    let resp = client
        .raw_request(
            RawMethod::Delete,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Delete user failed ({}): {msg}",
            resp.status
        )));
    }
    Ok(())
}

/// POST `/api/v1/users/{id}/password` — set/change a user's password.
#[server(prefix = "/api/manager")]
pub async fn manager_set_user_password(
    id: String,
    req: ManagerSetPasswordRequest,
) -> Result<(), ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let body =
        serde_json::to_vec(&req).map_err(|e| ServerFnError::new(format!("serialise: {e}")))?;
    let path = format!("/api/v1/users/{}/password", pct_encode_path(&id));
    let resp = client
        .raw_request(
            RawMethod::Post,
            &path,
            Some(&body),
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Set password failed ({}): {msg}",
            resp.status
        )));
    }
    Ok(())
}

// ============================================================================
// Variables — `/variables` page server fns
// ============================================================================

use crate::wire::variables::WireVariable;

/// Request body for `manager_create_variable`. Mirrors the daemon's
/// `CreateVariableRequest`. Kept local to avoid pulling `zlayer-api` into
/// the hydrate/WASM build.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagerCreateVariableRequest {
    /// Variable name (unique within the chosen scope).
    pub name: String,
    /// Plaintext value.
    pub value: String,
    /// Optional project-scope id. `None` = global variable.
    pub scope: Option<String>,
}

/// Patch-style update body. All fields optional.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ManagerUpdateVariableRequest {
    /// New name, or leave unchanged.
    pub name: Option<String>,
    /// New value, or leave unchanged.
    pub value: Option<String>,
}

/// GET `/api/v1/variables` — list all global variables.
#[server(prefix = "/api/manager")]
pub async fn manager_list_variables() -> Result<Vec<WireVariable>, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let resp = client
        .raw_request(
            RawMethod::Get,
            "/api/v1/variables",
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "List variables failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise variables: {e}")))
}

/// POST `/api/v1/variables` — create a new variable. Admin only on the
/// backend.
#[server(prefix = "/api/manager")]
pub async fn manager_create_variable(
    name: String,
    value: String,
    scope: Option<String>,
) -> Result<WireVariable, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let req = ManagerCreateVariableRequest { name, value, scope };
    let body =
        serde_json::to_vec(&req).map_err(|e| ServerFnError::new(format!("serialise: {e}")))?;
    let resp = client
        .raw_request(
            RawMethod::Post,
            "/api/v1/variables",
            Some(&body),
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Create variable failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise variable: {e}")))
}

/// PATCH `/api/v1/variables/{id}` — update value and/or name.
#[server(prefix = "/api/manager")]
pub async fn manager_update_variable(
    id: String,
    name: Option<String>,
    value: Option<String>,
) -> Result<WireVariable, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let req = ManagerUpdateVariableRequest { name, value };
    let body =
        serde_json::to_vec(&req).map_err(|e| ServerFnError::new(format!("serialise: {e}")))?;
    let path = format!("/api/v1/variables/{}", pct_encode_path(&id));
    let resp = client
        .raw_request(
            RawMethod::Patch,
            &path,
            Some(&body),
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Update variable failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise variable: {e}")))
}

/// DELETE `/api/v1/variables/{id}`. Returns `Ok(())` on 204/200.
#[server(prefix = "/api/manager")]
pub async fn manager_delete_variable(id: String) -> Result<(), ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = format!("/api/v1/variables/{}", pct_encode_path(&id));
    let resp = client
        .raw_request(
            RawMethod::Delete,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Delete variable failed ({}): {msg}",
            resp.status
        )));
    }
    Ok(())
}

// ============================================================================
// Secrets — `/secrets` page server fns
// ============================================================================

use crate::wire::secrets::{BulkImportResult, WireEnvironment, WireSecret};

/// Request body for `manager_create_secret`.
///
/// Mirrors the daemon's `CreateSecretRequest`. Note the daemon does NOT
/// accept a body-level `environment` field — that flows via the
/// `?environment=` query string instead. `scope` is only honoured in the
/// legacy path (mutually exclusive with the query env).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagerCreateSecretRequest {
    /// Secret identifier within the chosen scope.
    pub name: String,
    /// Plaintext value. Stored encrypted at rest.
    pub value: String,
}

/// GET `/api/v1/environments` — list environments. When
/// `include_all_projects` is true we request `?project=*` so the caller
/// sees globals + every project's envs in one table.
///
/// The Secrets page uses the default (globals only) because Phase 4
/// env-scoped secrets live on global environments; project-scoped envs
/// show up here only when `include_all_projects=true` is passed.
#[server(prefix = "/api/manager")]
pub async fn manager_list_environments() -> Result<Vec<WireEnvironment>, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    // Always request `?project=*` so the Secrets page can show secrets
    // attached to project envs in addition to globals. The backend sorts
    // them by name.
    let resp = client
        .raw_request(
            RawMethod::Get,
            "/api/v1/environments?project=*",
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "List environments failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise environments: {e}")))
}

/// GET `/api/v1/secrets` — list secret metadata. Without `environment`
/// we list the daemon's `default` scope.
#[server(prefix = "/api/manager")]
pub async fn manager_list_secrets(
    environment: Option<String>,
) -> Result<Vec<WireSecret>, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = match environment.as_deref() {
        Some(id) if !id.is_empty() => {
            format!("/api/v1/secrets?environment={}", pct_encode_path(id))
        }
        _ => "/api/v1/secrets".to_string(),
    };
    let resp = client
        .raw_request(
            RawMethod::Get,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "List secrets failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise secrets: {e}")))
}

/// POST `/api/v1/secrets` — upsert a secret. The daemon treats create and
/// update identically at this endpoint (201 vs 200 on the status code). We
/// expose a single helper for both flows.
#[server(prefix = "/api/manager")]
pub async fn manager_create_secret(
    name: String,
    value: String,
    environment: Option<String>,
) -> Result<WireSecret, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let req = ManagerCreateSecretRequest { name, value };
    let body =
        serde_json::to_vec(&req).map_err(|e| ServerFnError::new(format!("serialise: {e}")))?;
    let path = match environment.as_deref() {
        Some(id) if !id.is_empty() => {
            format!("/api/v1/secrets?environment={}", pct_encode_path(id))
        }
        _ => "/api/v1/secrets".to_string(),
    };
    let resp = client
        .raw_request(
            RawMethod::Post,
            &path,
            Some(&body),
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Create secret failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise secret: {e}")))
}

/// Upsert-style update. The daemon's secrets endpoint has no PATCH — POST
/// with the same name replaces the value. We expose it as `update_secret`
/// so the UI call site reads naturally.
#[server(prefix = "/api/manager")]
pub async fn manager_update_secret(
    name: String,
    value: String,
    environment: Option<String>,
) -> Result<WireSecret, ServerFnError> {
    manager_create_secret(name, value, environment).await
}

/// Reveal the plaintext value for a secret. Admin-only on the daemon —
/// non-admin sessions receive 403 and the helper surfaces the error.
///
/// We parse the full `SecretMetadataResponse` and extract `.value`, falling
/// back to an error if the server responded with metadata-only (which
/// would only happen if we mis-called the endpoint).
#[server(prefix = "/api/manager")]
pub async fn manager_reveal_secret(
    name: String,
    environment: Option<String>,
) -> Result<String, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = match environment.as_deref() {
        Some(id) if !id.is_empty() => format!(
            "/api/v1/secrets/{}?reveal=true&environment={}",
            pct_encode_path(&name),
            pct_encode_path(id),
        ),
        _ => format!("/api/v1/secrets/{}?reveal=true", pct_encode_path(&name)),
    };
    let resp = client
        .raw_request(
            RawMethod::Get,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Reveal secret failed ({}): {msg}",
            resp.status
        )));
    }
    let wire: WireSecret = serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise secret: {e}")))?;
    wire.value
        .ok_or_else(|| ServerFnError::new("server did not return a plaintext value".to_string()))
}

/// DELETE `/api/v1/secrets/{name}` scoped to the env (when provided).
#[server(prefix = "/api/manager")]
pub async fn manager_delete_secret(
    name: String,
    environment: Option<String>,
) -> Result<(), ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = match environment.as_deref() {
        Some(id) if !id.is_empty() => format!(
            "/api/v1/secrets/{}?environment={}",
            pct_encode_path(&name),
            pct_encode_path(id),
        ),
        _ => format!("/api/v1/secrets/{}", pct_encode_path(&name)),
    };
    let resp = client
        .raw_request(
            RawMethod::Delete,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Delete secret failed ({}): {msg}",
            resp.status
        )));
    }
    Ok(())
}

/// POST `/api/v1/secrets/bulk-import?environment={id}` — upload `.env`
/// content to an environment scope. The daemon requires a real env id (no
/// globals), so when the caller passes `None` we surface an error up
/// front rather than sending the request.
#[server(prefix = "/api/manager")]
pub async fn manager_bulk_import_secrets(
    environment: Option<String>,
    entries: Vec<(String, String)>,
) -> Result<BulkImportResult, ServerFnError> {
    use crate::api_client::RawMethod;
    let env_id = environment
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            ServerFnError::new(
                "Bulk import requires an environment id (the daemon rejects globals)".to_string(),
            )
        })?;

    // Serialise entries as `.env` lines. Values are written verbatim —
    // the daemon parses them with the same quote-stripping rules our
    // client applied.
    let mut body = String::new();
    for (k, v) in &entries {
        body.push_str(k);
        body.push('=');
        body.push_str(v);
        body.push('\n');
    }

    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = format!(
        "/api/v1/secrets/bulk-import?environment={}",
        pct_encode_path(env_id)
    );
    let resp = client
        .raw_request(
            RawMethod::Post,
            &path,
            Some(body.as_bytes()),
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Bulk import failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise bulk result: {e}")))
}

// ============================================================================
// Tasks — `/tasks` page server fns
// ============================================================================

use crate::wire::tasks::{WireTask, WireTaskRun, WireTaskSpec};

/// GET `/api/v1/tasks` — list all tasks (global + project-scoped).
#[server(prefix = "/api/manager")]
pub async fn manager_list_tasks() -> Result<Vec<WireTask>, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let resp = client
        .raw_request(
            RawMethod::Get,
            "/api/v1/tasks",
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "List tasks failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise tasks: {e}")))
}

/// GET `/api/v1/tasks/{id}` — fetch a single task.
#[server(prefix = "/api/manager")]
pub async fn manager_get_task(id: String) -> Result<WireTask, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = format!("/api/v1/tasks/{}", pct_encode_path(&id));
    let resp = client
        .raw_request(
            RawMethod::Get,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Get task failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise task: {e}")))
}

/// POST `/api/v1/tasks` — create a new task. Admin only on the backend.
///
/// `spec.kind` must be `"bash"` — that's the only variant the daemon
/// accepts today (`TaskKind::Bash`).
#[server(prefix = "/api/manager")]
pub async fn manager_create_task(spec: WireTaskSpec) -> Result<WireTask, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let body =
        serde_json::to_vec(&spec).map_err(|e| ServerFnError::new(format!("serialise: {e}")))?;
    let resp = client
        .raw_request(
            RawMethod::Post,
            "/api/v1/tasks",
            Some(&body),
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Create task failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise task: {e}")))
}

/// DELETE `/api/v1/tasks/{id}`. Returns `Ok(())` on 204/200.
#[server(prefix = "/api/manager")]
pub async fn manager_delete_task(id: String) -> Result<(), ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = format!("/api/v1/tasks/{}", pct_encode_path(&id));
    let resp = client
        .raw_request(
            RawMethod::Delete,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Delete task failed ({}): {msg}",
            resp.status
        )));
    }
    Ok(())
}

/// POST `/api/v1/tasks/{id}/run` — execute a task synchronously. Admin only.
///
/// This call blocks until the run completes; the daemon captures
/// stdout/stderr and the final exit code and returns the recorded
/// [`WireTaskRun`].
#[server(prefix = "/api/manager")]
pub async fn manager_run_task(id: String) -> Result<WireTaskRun, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = format!("/api/v1/tasks/{}/run", pct_encode_path(&id));
    let resp = client
        .raw_request(
            RawMethod::Post,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Run task failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise task run: {e}")))
}

/// GET `/api/v1/tasks/{id}/runs` — list past runs for a task, most-recent
/// first.
#[server(prefix = "/api/manager")]
pub async fn manager_list_task_runs(task_id: String) -> Result<Vec<WireTaskRun>, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = format!("/api/v1/tasks/{}/runs", pct_encode_path(&task_id));
    let resp = client
        .raw_request(
            RawMethod::Get,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "List task runs failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise task runs: {e}")))
}

// ============================================================================
// Notifiers — `/notifiers` page server fns
// ============================================================================

use crate::wire::notifiers::{NotifierTestResult, WireNotifier, WireNotifierConfig};

/// Body for `POST /api/v1/notifiers`. Matches the daemon's
/// `CreateNotifierRequest`: `{ name, kind, config }` where `kind` is the
/// lowercase discriminator and `config` is the tagged-union payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagerCreateNotifierRequest {
    /// Display name.
    pub name: String,
    /// One of `"slack"`, `"discord"`, `"webhook"`, `"smtp"`.
    pub kind: String,
    /// Channel-specific configuration; `type` must match `kind`.
    pub config: WireNotifierConfig,
}

/// Body for `PATCH /api/v1/notifiers/{id}` — all fields optional.
#[derive(Debug, Clone, Serialize, Default, Deserialize)]
pub struct ManagerUpdateNotifierRequest {
    /// New display name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// New enabled flag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// New configuration — `type` tag must match the existing notifier's
    /// kind (the daemon rejects kind/config mismatches with 400).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<WireNotifierConfig>,
}

/// GET `/api/v1/notifiers` — list every configured notifier.
#[server(prefix = "/api/manager")]
pub async fn manager_list_notifiers() -> Result<Vec<WireNotifier>, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let resp = client
        .raw_request(
            RawMethod::Get,
            "/api/v1/notifiers",
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "List notifiers failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise notifiers: {e}")))
}

/// POST `/api/v1/notifiers` — create a new notifier. Admin only on the
/// backend.
///
/// Daemon-side create always produces `enabled: true`; when the caller
/// asks for `enabled: false` we immediately PATCH the notifier so the
/// final state matches the form.
#[server(prefix = "/api/manager")]
pub async fn manager_create_notifier(
    name: String,
    enabled: bool,
    config: WireNotifierConfig,
) -> Result<WireNotifier, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();

    let kind = config.kind().to_string();
    let req = ManagerCreateNotifierRequest { name, kind, config };
    let body =
        serde_json::to_vec(&req).map_err(|e| ServerFnError::new(format!("serialise: {e}")))?;
    let resp = client
        .raw_request(
            RawMethod::Post,
            "/api/v1/notifiers",
            Some(&body),
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Create notifier failed ({}): {msg}",
            resp.status
        )));
    }
    let mut notifier: WireNotifier = serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise notifier: {e}")))?;
    if !enabled {
        notifier = manager_update_notifier(notifier.id.clone(), None, Some(false), None).await?;
    }
    Ok(notifier)
}

/// PATCH `/api/v1/notifiers/{id}`. Admin only on the backend.
#[server(prefix = "/api/manager")]
pub async fn manager_update_notifier(
    id: String,
    name: Option<String>,
    enabled: Option<bool>,
    config: Option<WireNotifierConfig>,
) -> Result<WireNotifier, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let req = ManagerUpdateNotifierRequest {
        name,
        enabled,
        config,
    };
    let body =
        serde_json::to_vec(&req).map_err(|e| ServerFnError::new(format!("serialise: {e}")))?;
    let path = format!("/api/v1/notifiers/{}", pct_encode_path(&id));
    let resp = client
        .raw_request(
            RawMethod::Patch,
            &path,
            Some(&body),
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Update notifier failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise notifier: {e}")))
}

/// DELETE `/api/v1/notifiers/{id}`. Admin only. Returns `Ok(())` on any
/// 2xx response.
#[server(prefix = "/api/manager")]
pub async fn manager_delete_notifier(id: String) -> Result<(), ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = format!("/api/v1/notifiers/{}", pct_encode_path(&id));
    let resp = client
        .raw_request(
            RawMethod::Delete,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Delete notifier failed ({}): {msg}",
            resp.status
        )));
    }
    Ok(())
}

/// POST `/api/v1/notifiers/{id}/test` — send a live test notification
/// through the configured channel. Admin only. Upstream failures are
/// returned as HTTP 200 with `success: false` per the daemon's contract,
/// so `Ok(NotifierTestResult { success: false, .. })` is a normal outcome
/// that the UI should surface as an `alert-error` rather than a
/// server-fn error.
#[server(prefix = "/api/manager")]
pub async fn manager_test_notifier(id: String) -> Result<NotifierTestResult, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = format!("/api/v1/notifiers/{}/test", pct_encode_path(&id));
    let resp = client
        .raw_request(
            RawMethod::Post,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Test notifier failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise test result: {e}")))
}

// ============================================================================
// Workflows — `/workflows` page server fns
// ============================================================================
//
// Endpoint inventory (from `zlayer-api/src/router.rs::build_workflow_routes`
// and `handlers/workflows.rs`):
//
//   GET    /api/v1/workflows             -> list
//   POST   /api/v1/workflows             -> create (admin)
//   GET    /api/v1/workflows/{id}        -> get
//   DELETE /api/v1/workflows/{id}        -> delete (admin)
//   POST   /api/v1/workflows/{id}/run    -> execute synchronously (admin)
//   GET    /api/v1/workflows/{id}/runs   -> list past runs
//
// No PATCH / update endpoint is exposed, so the UI omits an Edit action —
// same pattern Tasks uses. To change a workflow, delete and recreate.

use crate::wire::workflows::{WireWorkflow, WireWorkflowRun, WireWorkflowSpec};

/// GET `/api/v1/workflows` — list all workflows (global + project-scoped).
#[server(prefix = "/api/manager")]
pub async fn manager_list_workflows() -> Result<Vec<WireWorkflow>, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let resp = client
        .raw_request(
            RawMethod::Get,
            "/api/v1/workflows",
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "List workflows failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise workflows: {e}")))
}

/// GET `/api/v1/workflows/{id}` — fetch a single workflow.
#[server(prefix = "/api/manager")]
pub async fn manager_get_workflow(id: String) -> Result<WireWorkflow, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = format!("/api/v1/workflows/{}", pct_encode_path(&id));
    let resp = client
        .raw_request(
            RawMethod::Get,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Get workflow failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise workflow: {e}")))
}

/// POST `/api/v1/workflows` — create a new workflow. Admin only on the
/// backend. Body matches the daemon's `CreateWorkflowRequest` exactly:
/// `{ name, steps, project_id? }`.
#[server(prefix = "/api/manager")]
pub async fn manager_create_workflow(
    spec: WireWorkflowSpec,
) -> Result<WireWorkflow, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let body =
        serde_json::to_vec(&spec).map_err(|e| ServerFnError::new(format!("serialise: {e}")))?;
    let resp = client
        .raw_request(
            RawMethod::Post,
            "/api/v1/workflows",
            Some(&body),
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Create workflow failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise workflow: {e}")))
}

/// DELETE `/api/v1/workflows/{id}`. Admin only. Returns `Ok(())` on any
/// 2xx response; the daemon cascade-deletes the workflow's run history
/// atomically.
#[server(prefix = "/api/manager")]
pub async fn manager_delete_workflow(id: String) -> Result<(), ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = format!("/api/v1/workflows/{}", pct_encode_path(&id));
    let resp = client
        .raw_request(
            RawMethod::Delete,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Delete workflow failed ({}): {msg}",
            resp.status
        )));
    }
    Ok(())
}

/// POST `/api/v1/workflows/{id}/run` — execute a workflow synchronously.
/// Admin only.
///
/// This call blocks until the run completes; the daemon records the run
/// in `workflow_runs` and returns the terminal [`WireWorkflowRun`] with
/// per-step results. `BuildProject` steps can take several minutes when
/// the underlying buildah invocation is slow, so the UI should present a
/// spinner and avoid racing additional requests.
#[server(prefix = "/api/manager")]
pub async fn manager_run_workflow(id: String) -> Result<WireWorkflowRun, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = format!("/api/v1/workflows/{}/run", pct_encode_path(&id));
    let resp = client
        .raw_request(
            RawMethod::Post,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Run workflow failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise workflow run: {e}")))
}

/// GET `/api/v1/workflows/{id}/runs` — list past runs for a workflow,
/// most-recent first (the daemon returns a reverse-chronological index
/// scan on `(workflow_id, started_at DESC)`).
#[server(prefix = "/api/manager")]
pub async fn manager_list_workflow_runs(
    workflow_id: String,
) -> Result<Vec<WireWorkflowRun>, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = format!("/api/v1/workflows/{}/runs", pct_encode_path(&workflow_id));
    let resp = client
        .raw_request(
            RawMethod::Get,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "List workflow runs failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise workflow runs: {e}")))
}

// ============================================================================
// Groups — `/groups` page server fns
// ============================================================================
//
// Endpoint inventory (from `zlayer-api/src/router.rs::build_group_routes`
// and `handlers/groups.rs`):
//
//   GET    /api/v1/groups                         -> list
//   POST   /api/v1/groups                         -> create (admin)
//   GET    /api/v1/groups/{id}                    -> get
//   PATCH  /api/v1/groups/{id}                    -> update name/description (admin)
//   DELETE /api/v1/groups/{id}                    -> delete (admin)
//   POST   /api/v1/groups/{id}/members            -> add member (admin)
//   DELETE /api/v1/groups/{id}/members/{user_id}  -> remove member (admin)
//
// Read endpoints accept any authenticated actor; mutations require the
// `admin` role (enforced server-side).

use crate::wire::groups::WireUserGroup;
// `WireAddMember`, `WireCreateGroup`, `WireGroupMembers`, and
// `WireUpdateGroup` are only used inside `#[server]` bodies (which get
// stripped on the hydrate/WASM target), so the import is `cfg(ssr)` to
// avoid a `dead_code` warning on the client build.
#[cfg(feature = "ssr")]
use crate::wire::groups::{WireAddMember, WireCreateGroup, WireGroupMembers, WireUpdateGroup};

/// GET `/api/v1/groups` — list all groups.
#[server(prefix = "/api/manager")]
pub async fn manager_list_groups() -> Result<Vec<WireUserGroup>, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let resp = client
        .raw_request(
            RawMethod::Get,
            "/api/v1/groups",
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "List groups failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise groups: {e}")))
}

/// POST `/api/v1/groups` — create a new group. Admin only on the backend.
#[server(prefix = "/api/manager")]
pub async fn manager_create_group(
    name: String,
    description: Option<String>,
) -> Result<WireUserGroup, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let req = WireCreateGroup { name, description };
    let body =
        serde_json::to_vec(&req).map_err(|e| ServerFnError::new(format!("serialise: {e}")))?;
    let resp = client
        .raw_request(
            RawMethod::Post,
            "/api/v1/groups",
            Some(&body),
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Create group failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise group: {e}")))
}

/// PATCH `/api/v1/groups/{id}` — update group name/description. Admin only.
///
/// The daemon accepts an empty patch body as a no-op and returns the
/// unchanged group; the UI doesn't exercise that path (it only sends the
/// modal's edited fields) but the round-trip is symmetric.
#[server(prefix = "/api/manager")]
pub async fn manager_update_group(
    id: String,
    name: Option<String>,
    description: Option<String>,
) -> Result<WireUserGroup, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let req = WireUpdateGroup { name, description };
    let body =
        serde_json::to_vec(&req).map_err(|e| ServerFnError::new(format!("serialise: {e}")))?;
    let path = format!("/api/v1/groups/{}", pct_encode_path(&id));
    let resp = client
        .raw_request(
            RawMethod::Patch,
            &path,
            Some(&body),
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Update group failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise group: {e}")))
}

/// DELETE `/api/v1/groups/{id}` — cascading delete (also removes member rows).
/// Admin only. Returns `Ok(())` on 204/200.
#[server(prefix = "/api/manager")]
pub async fn manager_delete_group(id: String) -> Result<(), ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = format!("/api/v1/groups/{}", pct_encode_path(&id));
    let resp = client
        .raw_request(
            RawMethod::Delete,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Delete group failed ({}): {msg}",
            resp.status
        )));
    }
    Ok(())
}

/// GET `/api/v1/groups/{id}/members` — list user ids that belong to the
/// group. The daemon returns `{ group_id, members: [user_id, ...] }`.
#[server(prefix = "/api/manager")]
pub async fn manager_list_group_members(group_id: String) -> Result<Vec<String>, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = format!("/api/v1/groups/{}/members", pct_encode_path(&group_id));
    let resp = client
        .raw_request(
            RawMethod::Get,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "List group members failed ({}): {msg}",
            resp.status
        )));
    }
    let parsed: WireGroupMembers = serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise group members: {e}")))?;
    Ok(parsed.members)
}

/// POST `/api/v1/groups/{id}/members` — add a user to a group. Admin only.
#[server(prefix = "/api/manager")]
pub async fn manager_add_group_member(
    group_id: String,
    user_id: String,
) -> Result<(), ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let req = WireAddMember { user_id };
    let body =
        serde_json::to_vec(&req).map_err(|e| ServerFnError::new(format!("serialise: {e}")))?;
    let path = format!("/api/v1/groups/{}/members", pct_encode_path(&group_id));
    let resp = client
        .raw_request(
            RawMethod::Post,
            &path,
            Some(&body),
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Add group member failed ({}): {msg}",
            resp.status
        )));
    }
    Ok(())
}

/// DELETE `/api/v1/groups/{id}/members/{user_id}` — remove a user from a
/// group. Admin only. Returns `Ok(())` on 204/200.
#[server(prefix = "/api/manager")]
pub async fn manager_remove_group_member(
    group_id: String,
    user_id: String,
) -> Result<(), ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = format!(
        "/api/v1/groups/{}/members/{}",
        pct_encode_path(&group_id),
        pct_encode_path(&user_id),
    );
    let resp = client
        .raw_request(
            RawMethod::Delete,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Remove group member failed ({}): {msg}",
            resp.status
        )));
    }
    Ok(())
}

// ============================================================================
// Permissions — `/permissions` page server fns
// ============================================================================
//
// Endpoint inventory (from `zlayer-api/src/router.rs::build_permissions_routes`
// and `handlers/permissions.rs`):
//
//   GET    /api/v1/permissions?user={id}|group={id} -> list for ONE subject
//   POST   /api/v1/permissions                      -> grant (admin)
//   DELETE /api/v1/permissions/{id}                 -> revoke (admin)
//
// The daemon's filter surface is deliberately narrow: `GET` accepts
// EXACTLY one of `?user=<id>` or `?group=<id>` (bad request otherwise).
// There is no multi-field server-side filter on subject_kind +
// subject_id + resource_kind + resource_id. The page therefore fetches
// the union of all per-user and per-group lists on load and filters
// client-side when the user narrows by subject type / subject id /
// resource kind.

use crate::wire::permissions::{WireGrantPermissionRequest, WirePermission};

/// GET `/api/v1/permissions?user={id}` or `?group={id}` — list every
/// grant for one subject. The daemon requires exactly one of the two
/// parameters.
///
/// Callers that want the full list of all permissions iterate
/// `manager_list_users` + `manager_list_groups` and call this once per
/// subject, then concatenate on the client. That's what the Permissions
/// page does on mount.
#[server(prefix = "/api/manager")]
pub async fn manager_list_permissions_for_subject(
    subject_kind: String,
    subject_id: String,
) -> Result<Vec<WirePermission>, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let query_key = match subject_kind.as_str() {
        "user" => "user",
        "group" => "group",
        other => {
            return Err(ServerFnError::new(format!(
                "invalid subject_kind {other:?}; must be \"user\" or \"group\""
            )));
        }
    };
    // Simple query-param encoding — ids are UUIDs in practice, but still
    // pct-encode defensively in case the caller passes anything exotic.
    let path = format!(
        "/api/v1/permissions?{}={}",
        query_key,
        pct_encode_path(&subject_id)
    );
    let resp = client
        .raw_request(
            RawMethod::Get,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "List permissions failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise permissions: {e}")))
}

/// POST `/api/v1/permissions` — grant a permission. Admin only.
#[server(prefix = "/api/manager")]
pub async fn manager_grant_permission(
    req: WireGrantPermissionRequest,
) -> Result<WirePermission, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let body =
        serde_json::to_vec(&req).map_err(|e| ServerFnError::new(format!("serialise: {e}")))?;
    let resp = client
        .raw_request(
            RawMethod::Post,
            "/api/v1/permissions",
            Some(&body),
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Grant permission failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise permission: {e}")))
}

/// DELETE `/api/v1/permissions/{id}` — revoke a permission. Admin only.
/// Returns `Ok(())` on 204/200.
#[server(prefix = "/api/manager")]
pub async fn manager_revoke_permission(id: String) -> Result<(), ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = format!("/api/v1/permissions/{}", pct_encode_path(&id));
    let resp = client
        .raw_request(
            RawMethod::Delete,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Revoke permission failed ({}): {msg}",
            resp.status
        )));
    }
    Ok(())
}

// ============================================================================
// Audit — `/audit` page server fn (read-only, admin-only on the backend)
// ============================================================================
//
// Daemon surface (see `crates/zlayer-api/src/handlers/audit.rs`):
//
//   GET /api/v1/audit?user={id}&resource_kind={k}&since={ts}&until={ts}&limit={n}
//
// All query params are optional. `since` / `until` are RFC-3339 timestamps
// parsed by `chrono::DateTime<Utc>` on the daemon. Response is a bare
// `Vec<AuditEntry>` JSON array (no envelope), ordered newest-first.

use crate::wire::audit::{WireAuditEntry, WireAuditFilter};

/// Percent-encode a value destined for a URL query-string. Unlike
/// `pct_encode_path`, this encodes `+` (which some HTTP parsers treat as
/// a literal space in query strings) and keeps `=` / `&` out of the
/// encoded output. Anything that isn't in the unreserved set per RFC 3986
/// gets `%HH`-escaped.
#[cfg(feature = "ssr")]
fn pct_encode_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(&mut out, "%{b:02X}");
        }
    }
    out
}

/// GET `/api/v1/audit` — list audit entries matching `filter`. Admin only
/// on the backend. Returns the bare `Vec<WireAuditEntry>` array the daemon
/// emits (ordered newest-first).
///
/// Empty / `None` filter fields are skipped rather than sent as empty
/// query params (the daemon would deserialize `""` as `Some("")` which is
/// never what the caller wants).
#[server(prefix = "/api/manager")]
pub async fn manager_list_audit(
    filter: WireAuditFilter,
) -> Result<Vec<WireAuditEntry>, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();

    // Build the query string. Only non-empty / Some values are appended so
    // the server's `#[serde(default)] Option<_>` fields stay `None` rather
    // than `Some("")`.
    let mut params: Vec<String> = Vec::new();
    if let Some(u) = filter.user_id.as_deref().filter(|s| !s.is_empty()) {
        params.push(format!("user={}", pct_encode_query(u)));
    }
    if let Some(rk) = filter.resource_kind.as_deref().filter(|s| !s.is_empty()) {
        params.push(format!("resource_kind={}", pct_encode_query(rk)));
    }
    if let Some(since) = filter.since.as_deref().filter(|s| !s.is_empty()) {
        params.push(format!("since={}", pct_encode_query(since)));
    }
    if let Some(until) = filter.until.as_deref().filter(|s| !s.is_empty()) {
        params.push(format!("until={}", pct_encode_query(until)));
    }
    if let Some(lim) = filter.limit {
        params.push(format!("limit={lim}"));
    }
    let path = if params.is_empty() {
        "/api/v1/audit".to_string()
    } else {
        format!("/api/v1/audit?{}", params.join("&"))
    };

    let resp = client
        .raw_request(
            RawMethod::Get,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "List audit failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise audit: {e}")))
}

// ============================================================================
// Projects — `/projects` page server fns
// ============================================================================

use crate::wire::projects::{
    WireProject, WireProjectCredential, WireProjectSpec, WirePullResult, WireWebhookInfo,
};

/// GET `/api/v1/projects` — list every project.
#[server(prefix = "/api/manager")]
pub async fn manager_list_projects() -> Result<Vec<WireProject>, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let resp = client
        .raw_request(
            RawMethod::Get,
            "/api/v1/projects",
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "List projects failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise projects: {e}")))
}

/// GET `/api/v1/projects/{id}` — fetch a single project.
#[server(prefix = "/api/manager")]
pub async fn manager_get_project(id: String) -> Result<WireProject, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = format!("/api/v1/projects/{}", pct_encode_path(&id));
    let resp = client
        .raw_request(
            RawMethod::Get,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Get project failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise project: {e}")))
}

/// POST `/api/v1/projects` — create a new project. Admin only on the backend.
///
/// `spec.name` must be non-empty — the daemon returns 400 otherwise. Any
/// fields not set in `spec` are serialised as absent (via
/// `skip_serializing_if = "Option::is_none"`) so the daemon's
/// `#[serde(default)]` picks them up as `None`.
#[server(prefix = "/api/manager")]
pub async fn manager_create_project(spec: WireProjectSpec) -> Result<WireProject, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let body =
        serde_json::to_vec(&spec).map_err(|e| ServerFnError::new(format!("serialise: {e}")))?;
    let resp = client
        .raw_request(
            RawMethod::Post,
            "/api/v1/projects",
            Some(&body),
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Create project failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise project: {e}")))
}

/// PATCH `/api/v1/projects/{id}` — partial update. Admin only on the backend.
#[server(prefix = "/api/manager")]
pub async fn manager_update_project(
    id: String,
    spec: WireProjectSpec,
) -> Result<WireProject, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let body =
        serde_json::to_vec(&spec).map_err(|e| ServerFnError::new(format!("serialise: {e}")))?;
    let path = format!("/api/v1/projects/{}", pct_encode_path(&id));
    let resp = client
        .raw_request(
            RawMethod::Patch,
            &path,
            Some(&body),
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Update project failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise project: {e}")))
}

/// DELETE `/api/v1/projects/{id}` — cascade-deletes deployment links.
/// Admin only on the backend. Returns `Ok(())` on 204/200.
#[server(prefix = "/api/manager")]
pub async fn manager_delete_project(id: String) -> Result<(), ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = format!("/api/v1/projects/{}", pct_encode_path(&id));
    let resp = client
        .raw_request(
            RawMethod::Delete,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Delete project failed ({}): {msg}",
            resp.status
        )));
    }
    Ok(())
}

/// POST `/api/v1/projects/{id}/pull` — clone or fast-forward the project's
/// git working copy. Admin only on the backend.
///
/// This is synchronous — the call blocks until the clone / fetch completes.
#[server(prefix = "/api/manager")]
pub async fn manager_pull_project(id: String) -> Result<WirePullResult, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = format!("/api/v1/projects/{}/pull", pct_encode_path(&id));
    let resp = client
        .raw_request(
            RawMethod::Post,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Pull project failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise pull result: {e}")))
}

/// GET `/api/v1/projects/{id}/deployments` — list deployment names linked
/// to a project. The daemon returns a `Vec<String>` (deployment names only).
#[server(prefix = "/api/manager")]
pub async fn manager_list_project_deployments(id: String) -> Result<Vec<String>, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = format!("/api/v1/projects/{}/deployments", pct_encode_path(&id));
    let resp = client
        .raw_request(
            RawMethod::Get,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "List project deployments failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise project deployments: {e}")))
}

/// POST `/api/v1/projects/{id}/deployments` — link a deployment by name
/// to a project.
#[server(prefix = "/api/manager")]
pub async fn manager_link_project_deployment(
    id: String,
    deployment_name: String,
) -> Result<(), ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = format!("/api/v1/projects/{}/deployments", pct_encode_path(&id));
    let body = serde_json::json!({ "deployment_name": deployment_name });
    let body_vec =
        serde_json::to_vec(&body).map_err(|e| ServerFnError::new(format!("serialise: {e}")))?;
    let resp = client
        .raw_request(
            RawMethod::Post,
            &path,
            Some(&body_vec),
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Link project deployment failed ({}): {msg}",
            resp.status
        )));
    }
    Ok(())
}

/// DELETE `/api/v1/projects/{id}/deployments/{name}` — unlink a deployment
/// from a project.
#[server(prefix = "/api/manager")]
pub async fn manager_unlink_project_deployment(
    id: String,
    deployment_name: String,
) -> Result<(), ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = format!(
        "/api/v1/projects/{}/deployments/{}",
        pct_encode_path(&id),
        pct_encode_path(&deployment_name)
    );
    let resp = client
        .raw_request(
            RawMethod::Delete,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Unlink project deployment failed ({}): {msg}",
            resp.status
        )));
    }
    Ok(())
}

/// GET `/api/v1/projects/{id}/webhook` — fetch webhook URL + secret for a
/// project. Generates the secret on first call.
#[server(prefix = "/api/manager")]
pub async fn manager_get_project_webhook(id: String) -> Result<WireWebhookInfo, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = format!("/api/v1/projects/{}/webhook", pct_encode_path(&id));
    let resp = client
        .raw_request(
            RawMethod::Get,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Get webhook info failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise webhook info: {e}")))
}

/// POST `/api/v1/projects/{id}/webhook/rotate` — regenerate the webhook
/// secret. Admin only on the backend.
#[server(prefix = "/api/manager")]
pub async fn manager_rotate_project_webhook(id: String) -> Result<WireWebhookInfo, ServerFnError> {
    use crate::api_client::RawMethod;
    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();
    let path = format!("/api/v1/projects/{}/webhook/rotate", pct_encode_path(&id));
    let resp = client
        .raw_request(
            RawMethod::Post,
            &path,
            None,
            hdr.cookie.as_deref(),
            hdr.csrf.as_deref(),
        )
        .await
        .map_err(|e| api_error_to_server_error(&e))?;
    propagate_set_cookies(&resp.set_cookies);
    if !resp.status.is_success() {
        let msg = parse_error_message(&resp.body);
        return Err(ServerFnError::new(format!(
            "Rotate webhook failed ({}): {msg}",
            resp.status
        )));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ServerFnError::new(format!("deserialise webhook info: {e}")))
}

/// GET `/api/v1/credentials/registry` + `/api/v1/credentials/git`, merged
/// into the Project-view `WireProjectCredential` shape.
///
/// `kind_filter` narrows to one of `"registry"`, `"git"`, or (when
/// `None` / other) both. This server fn does one or two requests
/// depending on the filter.
#[server(prefix = "/api/manager")]
pub async fn manager_list_credentials(
    kind_filter: Option<String>,
) -> Result<Vec<WireProjectCredential>, ServerFnError> {
    use crate::api_client::RawMethod;

    // Local parse shapes — defined up front to keep the function body flat and
    // avoid `items_after_statements` under `--all-features` clippy.
    #[derive(Deserialize)]
    struct RegItem {
        id: String,
        registry: String,
        username: String,
        auth_type: String,
    }
    #[derive(Deserialize)]
    struct GitItem {
        id: String,
        name: String,
        kind: String,
    }

    let hdr = extract_forwarded_headers().await;
    let client = get_api_client();

    let kind = kind_filter.as_deref().unwrap_or("");
    let want_registry = kind.is_empty() || kind == "registry";
    let want_git = kind.is_empty() || kind == "git";

    let mut out: Vec<WireProjectCredential> = Vec::new();

    if want_registry {
        let resp = client
            .raw_request(
                RawMethod::Get,
                "/api/v1/credentials/registry",
                None,
                hdr.cookie.as_deref(),
                hdr.csrf.as_deref(),
            )
            .await
            .map_err(|e| api_error_to_server_error(&e))?;
        if !resp.status.is_success() {
            let msg = parse_error_message(&resp.body);
            return Err(ServerFnError::new(format!(
                "List registry credentials failed ({}): {msg}",
                resp.status
            )));
        }
        let items: Vec<RegItem> = serde_json::from_slice(&resp.body)
            .map_err(|e| ServerFnError::new(format!("deserialise registry credentials: {e}")))?;
        for it in items {
            out.push(WireProjectCredential {
                id: it.id,
                kind: "registry".to_string(),
                name: format!("{}  ({})", it.registry, it.username),
                sub_kind: it.auth_type,
            });
        }
    }

    if want_git {
        let resp = client
            .raw_request(
                RawMethod::Get,
                "/api/v1/credentials/git",
                None,
                hdr.cookie.as_deref(),
                hdr.csrf.as_deref(),
            )
            .await
            .map_err(|e| api_error_to_server_error(&e))?;
        if !resp.status.is_success() {
            let msg = parse_error_message(&resp.body);
            return Err(ServerFnError::new(format!(
                "List git credentials failed ({}): {msg}",
                resp.status
            )));
        }
        let items: Vec<GitItem> = serde_json::from_slice(&resp.body)
            .map_err(|e| ServerFnError::new(format!("deserialise git credentials: {e}")))?;
        for it in items {
            out.push(WireProjectCredential {
                id: it.id,
                kind: "git".to_string(),
                name: it.name,
                sub_kind: it.kind,
            });
        }
    }

    Ok(out)
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
            runtime_name: "youki".to_string(),
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
        assert!(json.contains("\"runtime_name\":\"youki\""));
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
            id: "1".to_string(),
            address: "192.168.1.10".to_string(),
            status: "ready".to_string(),
            role: "leader".to_string(),
            mode: "full".to_string(),
            is_leader: true,
            overlay_ip: "10.200.0.1".to_string(),
            cpu_total: 8.0,
            cpu_used: 2.0,
            memory_total: 16_000_000_000,
            memory_used: 4_000_000_000,
            registered_at: 1_706_745_600_000,
            last_heartbeat: 1_706_745_700_000,
        };
        let json = serde_json::to_string(&node).unwrap();
        assert!(json.contains("\"id\":\"1\""));
        assert!(json.contains("192.168.1.10"));
        assert!(json.contains("ready"));
        assert!(json.contains("leader"));
        assert!(json.contains("10.200.0.1"));
    }

    #[test]
    fn test_node_deserialize() {
        let json = r#"{
            "id": "2",
            "address": "192.168.1.11",
            "status": "dead",
            "role": "voter",
            "mode": "full",
            "is_leader": false,
            "overlay_ip": "10.200.0.2",
            "cpu_total": 16.0,
            "cpu_used": 8.0,
            "memory_total": 32000000000,
            "memory_used": 16000000000,
            "registered_at": 1706745600000,
            "last_heartbeat": 1706745700000
        }"#;
        let node: Node = serde_json::from_str(json).unwrap();
        assert_eq!(node.id, "2");
        assert_eq!(node.status, "dead");
        assert_eq!(node.role, "voter");
        assert!(!node.is_leader);
        assert_eq!(node.overlay_ip, "10.200.0.2");
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
            endpoints: vec![ServiceEndpoint {
                name: "http".to_string(),
                protocol: "http".to_string(),
                port: 8080,
                url: Some("http://web.my-app.local:8080".to_string()),
            }],
        };
        let json = serde_json::to_string(&service).unwrap();
        assert!(json.contains("web"));
        assert!(json.contains("my-app"));
        assert!(json.contains("8080"));
    }

    #[test]
    fn test_service_deserialize() {
        let json = r#"{
            "name": "db",
            "deployment": "backend",
            "status": "pending",
            "replicas": 0,
            "desired_replicas": 1,
            "endpoints": []
        }"#;
        let service: Service = serde_json::from_str(json).unwrap();
        assert_eq!(service.name, "db");
        assert_eq!(service.deployment, "backend");
        assert_eq!(service.status, "pending");
        assert!(service.endpoints.is_empty());
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
            runtime_name: "docker".to_string(),
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
        assert_eq!(original.runtime_name, restored.runtime_name);
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

    // =========================================================================
    // ProxyBackendInfo Tests
    // =========================================================================

    #[test]
    fn test_proxy_backend_info_serialize() {
        let backend = ProxyBackendInfo {
            address: "10.0.0.5:8080".to_string(),
            healthy: true,
            active_connections: 42,
        };
        let json = serde_json::to_string(&backend).unwrap();
        assert!(json.contains("10.0.0.5:8080"));
        assert!(json.contains("\"healthy\":true"));
        assert!(json.contains("\"active_connections\":42"));
    }

    #[test]
    fn test_proxy_backend_info_deserialize() {
        let json = r#"{
            "address": "10.0.0.6:9090",
            "healthy": false,
            "active_connections": 0
        }"#;
        let backend: ProxyBackendInfo = serde_json::from_str(json).unwrap();
        assert_eq!(backend.address, "10.0.0.6:9090");
        assert!(!backend.healthy);
        assert_eq!(backend.active_connections, 0);
    }

    // =========================================================================
    // ProxyRouteInfo Tests
    // =========================================================================

    #[test]
    fn test_proxy_route_info_serialize() {
        let route = ProxyRouteInfo {
            host: Some("api.example.com".to_string()),
            path_prefix: "/v1".to_string(),
            strip_prefix: true,
            backends: vec![ProxyBackendInfo {
                address: "10.0.0.5:8080".to_string(),
                healthy: true,
                active_connections: 10,
            }],
        };
        let json = serde_json::to_string(&route).unwrap();
        assert!(json.contains("api.example.com"));
        assert!(json.contains("/v1"));
        assert!(json.contains("\"strip_prefix\":true"));
    }

    #[test]
    fn test_proxy_route_info_deserialize_no_host() {
        let json = r#"{
            "path_prefix": "/api",
            "strip_prefix": false,
            "backends": []
        }"#;
        let route: ProxyRouteInfo = serde_json::from_str(json).unwrap();
        assert!(route.host.is_none());
        assert_eq!(route.path_prefix, "/api");
        assert!(!route.strip_prefix);
        assert!(route.backends.is_empty());
    }

    // =========================================================================
    // ProxyBackendGroupInfo Tests
    // =========================================================================

    #[test]
    fn test_proxy_backend_group_info_serialize() {
        let group = ProxyBackendGroupInfo {
            service: "web-app".to_string(),
            strategy: "round_robin".to_string(),
            backends: vec![
                ProxyBackendInfo {
                    address: "10.0.0.1:8080".to_string(),
                    healthy: true,
                    active_connections: 5,
                },
                ProxyBackendInfo {
                    address: "10.0.0.2:8080".to_string(),
                    healthy: true,
                    active_connections: 3,
                },
            ],
        };
        let json = serde_json::to_string(&group).unwrap();
        assert!(json.contains("web-app"));
        assert!(json.contains("round_robin"));
        assert!(json.contains("10.0.0.1:8080"));
        assert!(json.contains("10.0.0.2:8080"));
    }

    #[test]
    fn test_proxy_backend_group_info_deserialize() {
        let json = r#"{
            "service": "api-svc",
            "strategy": "least_connections",
            "backends": []
        }"#;
        let group: ProxyBackendGroupInfo = serde_json::from_str(json).unwrap();
        assert_eq!(group.service, "api-svc");
        assert_eq!(group.strategy, "least_connections");
        assert!(group.backends.is_empty());
    }

    // =========================================================================
    // TlsCertificateInfo Tests
    // =========================================================================

    #[test]
    fn test_tls_certificate_info_serialize() {
        let cert = TlsCertificateInfo {
            domain: "example.com".to_string(),
            issuer: Some("Let's Encrypt".to_string()),
            expires_at: Some("2026-01-01T00:00:00Z".to_string()),
            auto_renew: true,
        };
        let json = serde_json::to_string(&cert).unwrap();
        assert!(json.contains("example.com"));
        assert!(json.contains("Let's Encrypt"));
        assert!(json.contains("\"auto_renew\":true"));
    }

    #[test]
    fn test_tls_certificate_info_deserialize_minimal() {
        let json = r#"{
            "domain": "test.local",
            "auto_renew": false
        }"#;
        let cert: TlsCertificateInfo = serde_json::from_str(json).unwrap();
        assert_eq!(cert.domain, "test.local");
        assert!(cert.issuer.is_none());
        assert!(cert.expires_at.is_none());
        assert!(!cert.auto_renew);
    }

    // =========================================================================
    // StreamProxyInfo Tests
    // =========================================================================

    #[test]
    fn test_stream_proxy_info_serialize() {
        let stream = StreamProxyInfo {
            protocol: "tcp".to_string(),
            listen_port: 5432,
            backends: vec![ProxyBackendInfo {
                address: "10.0.0.10:5432".to_string(),
                healthy: true,
                active_connections: 20,
            }],
        };
        let json = serde_json::to_string(&stream).unwrap();
        assert!(json.contains("\"protocol\":\"tcp\""));
        assert!(json.contains("\"listen_port\":5432"));
        assert!(json.contains("10.0.0.10:5432"));
    }

    #[test]
    fn test_stream_proxy_info_deserialize() {
        let json = r#"{
            "protocol": "udp",
            "listen_port": 53,
            "backends": []
        }"#;
        let stream: StreamProxyInfo = serde_json::from_str(json).unwrap();
        assert_eq!(stream.protocol, "udp");
        assert_eq!(stream.listen_port, 53);
        assert!(stream.backends.is_empty());
    }

    // =========================================================================
    // Proxy Round-trip Tests
    // =========================================================================

    #[test]
    fn test_proxy_route_info_roundtrip() {
        let original = ProxyRouteInfo {
            host: Some("app.example.com".to_string()),
            path_prefix: "/api/v2".to_string(),
            strip_prefix: true,
            backends: vec![ProxyBackendInfo {
                address: "10.0.0.1:3000".to_string(),
                healthy: true,
                active_connections: 7,
            }],
        };
        let json = serde_json::to_string(&original).unwrap();
        let restored: ProxyRouteInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(original.host, restored.host);
        assert_eq!(original.path_prefix, restored.path_prefix);
        assert_eq!(original.strip_prefix, restored.strip_prefix);
        assert_eq!(original.backends.len(), restored.backends.len());
    }

    #[test]
    fn test_tls_certificate_info_roundtrip() {
        let original = TlsCertificateInfo {
            domain: "secure.example.com".to_string(),
            issuer: Some("ZeroSSL".to_string()),
            expires_at: Some("2026-06-15T00:00:00Z".to_string()),
            auto_renew: true,
        };
        let json = serde_json::to_string(&original).unwrap();
        let restored: TlsCertificateInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(original.domain, restored.domain);
        assert_eq!(original.issuer, restored.issuer);
        assert_eq!(original.expires_at, restored.expires_at);
        assert_eq!(original.auto_renew, restored.auto_renew);
    }

    #[test]
    fn test_stream_proxy_info_roundtrip() {
        let original = StreamProxyInfo {
            protocol: "tcp".to_string(),
            listen_port: 3306,
            backends: vec![
                ProxyBackendInfo {
                    address: "10.0.0.1:3306".to_string(),
                    healthy: true,
                    active_connections: 15,
                },
                ProxyBackendInfo {
                    address: "10.0.0.2:3306".to_string(),
                    healthy: false,
                    active_connections: 0,
                },
            ],
        };
        let json = serde_json::to_string(&original).unwrap();
        let restored: StreamProxyInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(original.protocol, restored.protocol);
        assert_eq!(original.listen_port, restored.listen_port);
        assert_eq!(original.backends.len(), restored.backends.len());
        assert_eq!(original.backends[0].address, restored.backends[0].address);
        assert_eq!(original.backends[1].healthy, restored.backends[1].healthy);
    }
}
