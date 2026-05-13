//! Internal API endpoints for scheduler-to-agent communication
//!
//! These endpoints are used by the distributed scheduler to trigger operations
//! on agents. They use a shared secret for authentication rather than JWT tokens.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{FromRequestParts, State},
    http::{header::HeaderValue, request::Parts, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{error, info, warn};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::error::{ApiError, Result};
use zlayer_agent::{AgentError, ServiceManager};
use zlayer_scheduler::RaftCoordinator;
pub use zlayer_types::api::internal::*;

/// Header name for internal API authentication
pub const INTERNAL_AUTH_HEADER: &str = "X-ZLayer-Internal-Token";

/// State for internal endpoints
#[derive(Clone)]
pub struct InternalState {
    /// Service manager for container lifecycle operations
    pub service_manager: Arc<RwLock<ServiceManager>>,
    /// Shared secret for authenticating internal calls
    pub internal_token: String,
    /// `WireGuard` overlay interface name (e.g. "zl-overlay0") for add-peer operations.
    /// `None` if overlay networking is not configured.
    pub overlay_interface: Option<String>,
    /// In-memory map of in-flight / completed daemon-binary upgrade jobs
    /// (keyed by `upgrade_id`).
    ///
    /// This is intentionally non-persistent: an upgrade either completes
    /// before the daemon restarts (in which case the new process starts
    /// with an empty map) or fails before restart (in which case the
    /// failure is reflected here for the leader to poll). Crash recovery
    /// of an in-flight upgrade is handled by the leader retrying via
    /// `/api/v1/cluster/upgrade`.
    pub upgrade_jobs: Arc<RwLock<HashMap<String, UpgradeJobState>>>,
    /// Optional data directory for writing the restart sentinel that the
    /// supervisor (or `--restart-on-exit` wrapper) consults after a clean
    /// exit. `None` disables the sentinel write — the supervisor must
    /// already restart the daemon unconditionally on exit code 75 in that
    /// case.
    pub data_dir: Option<std::path::PathBuf>,
    /// Optional Raft coordinator handle, used by
    /// `internal_raft_trigger_elect` to ask the local node to campaign
    /// before the leader self-upgrades. `None` on non-clustered daemons.
    pub raft: Option<Arc<RaftCoordinator>>,
}

impl InternalState {
    /// Create a new internal state
    pub fn new(service_manager: Arc<RwLock<ServiceManager>>, internal_token: String) -> Self {
        Self {
            service_manager,
            internal_token,
            overlay_interface: None,
            upgrade_jobs: Arc::new(RwLock::new(HashMap::new())),
            data_dir: None,
            raft: None,
        }
    }

    /// Create a new internal state with overlay interface for peer management
    pub fn with_overlay(
        service_manager: Arc<RwLock<ServiceManager>>,
        internal_token: String,
        overlay_interface: Option<String>,
    ) -> Self {
        Self {
            service_manager,
            internal_token,
            overlay_interface,
            upgrade_jobs: Arc::new(RwLock::new(HashMap::new())),
            data_dir: None,
            raft: None,
        }
    }

    /// Attach a Raft coordinator handle used by the pre-self-upgrade
    /// "nudge a follower to campaign" path (`internal_raft_trigger_elect`).
    #[must_use]
    pub fn with_raft(mut self, raft: Arc<RaftCoordinator>) -> Self {
        self.raft = Some(raft);
        self
    }

    /// Attach a data directory used for writing the post-upgrade restart
    /// sentinel (`{data_dir}/run/zlayer.restart`).
    #[must_use]
    pub fn with_data_dir(mut self, data_dir: std::path::PathBuf) -> Self {
        self.data_dir = Some(data_dir);
        self
    }
}

// =============================================================================
// Daemon-binary upgrade (used by `zlayer node upgrade`)
// =============================================================================

/// Lifecycle of a daemon-binary upgrade job, tracked in-memory on the node
/// being upgraded.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpgradeStatus {
    /// Job has been registered; the worker task has not yet started.
    Pending,
    /// Downloading the target release binary.
    Downloading,
    /// Replacing the on-disk binary with the new version.
    Applying,
    /// Daemon is about to exit so the supervisor can respawn it.
    Restarting,
    /// Upgrade failed before the restart was triggered. The `error` field
    /// on [`UpgradeJobState`] explains why.
    Failed,
}

/// State of a single daemon-binary upgrade attempt. Stored in
/// [`InternalState::upgrade_jobs`] and returned by `GET
/// /api/v1/internal/upgrade/{id}`.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct UpgradeJobState {
    /// Server-generated upgrade id (UUID v4).
    pub upgrade_id: String,
    /// Target version (e.g. `"v0.12.0"`); `None` means "latest release".
    pub version: Option<String>,
    /// Current lifecycle state.
    pub status: UpgradeStatus,
    /// When the job was registered.
    #[schema(value_type = String, format = "date-time")]
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// When the job entered its terminal state (`Restarting` or `Failed`).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>, format = "date-time")]
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Human-readable failure reason (set only when `status == Failed`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Request body for `POST /api/v1/internal/upgrade/start`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpgradeStartRequest {
    /// Target version. `None` (or `"latest"`) defers to `self-update`'s
    /// "latest GitHub release" resolver.
    #[serde(default)]
    pub version: Option<String>,
}

/// Response body for `POST /api/v1/internal/upgrade/start`.
#[derive(Debug, Serialize, ToSchema)]
pub struct UpgradeStartResponse {
    pub upgrade_id: String,
    pub message: String,
}

/// Schedule a daemon-binary upgrade on this node.
///
/// `POST /api/v1/internal/upgrade/start`
///
/// Registers an upgrade job, spawns a worker task that runs the
/// `zlayer self-update` subcommand against `current_exe()`, and returns
/// `202 Accepted` with an `upgrade_id` the caller can poll via
/// `internal_upgrade_status`. On success the daemon exits with code 75 so
/// the supervisor (or `--restart-on-exit`) respawns it.
///
/// # Errors
///
/// Returns `Unauthorized` if the internal token is missing or wrong.
#[utoipa::path(
    post,
    path = "/api/v1/internal/upgrade/start",
    request_body = UpgradeStartRequest,
    responses(
        (status = 202, description = "Upgrade scheduled", body = UpgradeStartResponse),
        (status = 401, description = "Unauthorized — invalid or missing internal token"),
    ),
    tag = "Internal"
)]
#[allow(clippy::too_many_lines)]
pub async fn internal_upgrade_start(
    _auth: InternalAuth,
    State(state): State<InternalState>,
    Json(req): Json<UpgradeStartRequest>,
) -> Result<(StatusCode, Json<UpgradeStartResponse>)> {
    let upgrade_id = Uuid::new_v4().to_string();
    let job = UpgradeJobState {
        upgrade_id: upgrade_id.clone(),
        version: req.version.clone(),
        status: UpgradeStatus::Pending,
        started_at: chrono::Utc::now(),
        finished_at: None,
        error: None,
    };
    {
        let mut jobs = state.upgrade_jobs.write().await;
        jobs.insert(upgrade_id.clone(), job);
    }

    // Spawn the worker. Returning `202 Accepted` first lets the response
    // leave before we potentially kill our own process below.
    let worker_state = state.clone();
    let worker_id = upgrade_id.clone();
    let worker_version = req.version.clone();
    tokio::spawn(async move {
        // Give the HTTP response a beat to escape the socket before we
        // start swapping our own binary out from under ourselves.
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        // Helper closures to bump the status field in one short critical section.
        let update_status = |status: UpgradeStatus, error: Option<String>| {
            let jobs = worker_state.upgrade_jobs.clone();
            let id = worker_id.clone();
            async move {
                let mut guard = jobs.write().await;
                if let Some(job) = guard.get_mut(&id) {
                    let terminal =
                        matches!(status, UpgradeStatus::Restarting | UpgradeStatus::Failed);
                    job.status = status;
                    if let Some(msg) = error {
                        job.error = Some(msg);
                    }
                    if terminal {
                        job.finished_at = Some(chrono::Utc::now());
                    }
                }
            }
        };

        // 1. Status -> Downloading. Run `zlayer self-update`.
        update_status(UpgradeStatus::Downloading, None).await;

        let current_exe = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                error!(error = %e, "internal_upgrade_start: failed to resolve current_exe");
                update_status(
                    UpgradeStatus::Failed,
                    Some(format!("current_exe lookup failed: {e}")),
                )
                .await;
                return;
            }
        };

        let mut cmd = tokio::process::Command::new(&current_exe);
        cmd.arg("self-update").arg("--yes");
        if let Some(v) = worker_version.as_deref() {
            if !v.is_empty() && v != "latest" {
                cmd.arg("--version").arg(v);
            }
        }

        info!(
            upgrade_id = %worker_id,
            exe = %current_exe.display(),
            version = ?worker_version,
            "internal_upgrade_start: spawning self-update subprocess"
        );

        let output = match cmd.output().await {
            Ok(o) => o,
            Err(e) => {
                error!(
                    upgrade_id = %worker_id,
                    error = %e,
                    "internal_upgrade_start: self-update subprocess failed to start"
                );
                update_status(
                    UpgradeStatus::Failed,
                    Some(format!("self-update spawn failed: {e}")),
                )
                .await;
                return;
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!(
                upgrade_id = %worker_id,
                exit_status = ?output.status,
                stderr = %stderr,
                "internal_upgrade_start: self-update subprocess returned non-zero"
            );
            update_status(
                UpgradeStatus::Failed,
                Some(format!(
                    "self-update exited with {:?}: {}",
                    output.status, stderr
                )),
            )
            .await;
            return;
        }

        // 2. Status -> Applying. (self-update covers download+apply in one shot;
        // we treat the post-success window as `Applying` then `Restarting` so
        // pollers see a sane progression.)
        update_status(UpgradeStatus::Applying, None).await;

        // 3. Write the restart sentinel if a data_dir was configured.
        if let Some(ref data_dir) = worker_state.data_dir {
            let run_dir = data_dir.join("run");
            if let Err(e) = tokio::fs::create_dir_all(&run_dir).await {
                warn!(
                    upgrade_id = %worker_id,
                    error = %e,
                    "internal_upgrade_start: failed to create run/ dir for restart sentinel"
                );
            } else {
                let sentinel = run_dir.join("zlayer.restart");
                if let Err(e) = tokio::fs::write(&sentinel, b"restart\n").await {
                    warn!(
                        upgrade_id = %worker_id,
                        error = %e,
                        path = %sentinel.display(),
                        "internal_upgrade_start: failed to write restart sentinel"
                    );
                }
            }
        }

        // 4. Status -> Restarting. Exit 75 = EX_TEMPFAIL, which is the
        // documented "respawn me" signal to the supervisor / wrapper.
        update_status(UpgradeStatus::Restarting, None).await;
        info!(
            upgrade_id = %worker_id,
            "internal_upgrade_start: exiting with code 75 so the supervisor respawns the daemon"
        );

        // Tiny grace period so the status write is observable before exit.
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        std::process::exit(75);
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(UpgradeStartResponse {
            upgrade_id,
            message: "Upgrade scheduled".to_string(),
        }),
    ))
}

/// Fetch the status of a previously-scheduled daemon-binary upgrade.
///
/// `GET /api/v1/internal/upgrade/{upgrade_id}`
///
/// # Errors
///
/// Returns `Unauthorized` if the internal token is missing or wrong, or
/// `NotFound` if `upgrade_id` is not in the in-memory job map (which is
/// expected after a daemon restart — callers should treat that as
/// "upgrade likely complete; daemon respawned").
#[utoipa::path(
    get,
    path = "/api/v1/internal/upgrade/{upgrade_id}",
    params(
        ("upgrade_id" = String, Path, description = "Upgrade job id returned by internal_upgrade_start"),
    ),
    responses(
        (status = 200, description = "Current upgrade state", body = UpgradeJobState),
        (status = 401, description = "Unauthorized — invalid or missing internal token"),
        (status = 404, description = "Upgrade id not found (may have been lost across a daemon restart)"),
    ),
    tag = "Internal"
)]
pub async fn internal_upgrade_status(
    _auth: InternalAuth,
    State(state): State<InternalState>,
    axum::extract::Path(upgrade_id): axum::extract::Path<String>,
) -> Result<Json<UpgradeJobState>> {
    let jobs = state.upgrade_jobs.read().await;
    let job = jobs
        .get(&upgrade_id)
        .ok_or_else(|| ApiError::NotFound(format!("upgrade id '{upgrade_id}' not found")))?;
    Ok(Json(job.clone()))
}

/// Trigger an immediate Raft election on this node.
///
/// `POST /api/v1/internal/raft/trigger-elect`
///
/// Called by the cluster leader's pre-self-upgrade flow on a healthy
/// follower: that follower campaigns immediately instead of waiting for
/// heartbeat-loss timeout after the leader exits. Raft safety still
/// holds — only an up-to-date candidate can win — so a stale callee
/// just loses the term and a more-up-to-date follower wins the next.
///
/// # Errors
///
/// - `Unauthorized` if the internal token is missing or wrong.
/// - `ServiceUnavailable` on non-clustered daemons (no Raft coordinator).
/// - `Internal` if the underlying `Raft::trigger().elect()` returns
///   `Fatal` (coordinator shutdown / storage failure).
#[utoipa::path(
    post,
    path = "/api/v1/internal/raft/trigger-elect",
    responses(
        (status = 202, description = "Election triggered"),
        (status = 401, description = "Unauthorized — invalid or missing internal token"),
        (status = 500, description = "Raft trigger_elect failed"),
        (status = 503, description = "Raft coordinator not configured on this daemon"),
    ),
    tag = "Internal"
)]
pub async fn internal_raft_trigger_elect(
    _auth: InternalAuth,
    State(state): State<InternalState>,
) -> Result<StatusCode> {
    let raft = state.raft.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable(
            "Raft coordinator not configured; trigger-elect is only available on clustered daemons"
                .into(),
        )
    })?;
    raft.trigger_elect()
        .await
        .map_err(|e| ApiError::Internal(format!("trigger_elect failed: {e}")))?;
    info!("internal_raft_trigger_elect: local node will campaign now");
    Ok(StatusCode::ACCEPTED)
}

/// Internal authentication extractor
///
/// Validates the X-ZLayer-Internal-Token header against the configured secret.
pub struct InternalAuth;

impl<S> FromRequestParts<S> for InternalAuth
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        // Get internal state from extensions
        let internal_state = parts
            .extensions
            .get::<InternalState>()
            .cloned()
            .ok_or_else(|| ApiError::Internal("Internal state not configured".to_string()))?;

        // Extract the internal token header
        let token = parts
            .headers
            .get(INTERNAL_AUTH_HEADER)
            .and_then(|value: &HeaderValue| value.to_str().ok())
            .ok_or_else(|| {
                warn!("Missing internal authentication header");
                ApiError::Unauthorized(format!("Missing {INTERNAL_AUTH_HEADER} header"))
            })?;

        // Verify the token
        if token != internal_state.internal_token {
            warn!("Invalid internal authentication token");
            return Err(ApiError::Unauthorized("Invalid internal token".to_string()));
        }

        Ok(InternalAuth)
    }
}

/// Scale a service via internal scheduler request.
///
/// This endpoint is called by the distributed scheduler leader to trigger
/// scaling operations on agent nodes. It uses a shared secret for authentication.
///
/// # Errors
///
/// Returns an error if the service is not found, scaling fails, or authentication
/// is invalid.
#[allow(clippy::cast_possible_truncation)]
#[utoipa::path(
    post,
    path = "/api/v1/internal/scale",
    request_body = InternalScaleRequest,
    responses(
        (status = 200, description = "Service scaled successfully", body = InternalScaleResponse),
        (status = 401, description = "Unauthorized - invalid or missing internal token"),
        (status = 404, description = "Service not found"),
        (status = 500, description = "Internal error"),
    ),
    tag = "Internal"
)]
pub async fn scale_service_internal(
    _auth: InternalAuth,
    State(state): State<InternalState>,
    Json(request): Json<InternalScaleRequest>,
) -> Result<Json<InternalScaleResponse>> {
    // Validate replica count
    if request.replicas > 100 {
        return Err(ApiError::BadRequest(
            "Replica count cannot exceed 100".to_string(),
        ));
    }

    info!(
        service = %request.service,
        replicas = request.replicas,
        "Internal scale request received"
    );

    // Get the service manager
    let manager = state.service_manager.read().await;

    // Check if service exists
    if manager
        .service_replica_count(&request.service)
        .await
        .is_err()
    {
        return Err(ApiError::NotFound(format!(
            "Service '{}' not found or not registered",
            request.service
        )));
    }

    // Scale the service. H-7: if the composite runtime reports
    // `RouteToPeer`, surface it as a structured response (not a 500) so the
    // scheduler can re-dispatch to a capable peer instead of treating the
    // service as broken.
    match manager
        .scale_service(&request.service, request.replicas)
        .await
    {
        Ok(()) => {}
        Err(AgentError::RouteToPeer {
            service,
            required_os,
            reason,
        }) => {
            warn!(
                service = %service,
                required_os = %required_os,
                reason = %reason,
                "this node cannot run the workload; signalling scheduler to re-place"
            );
            return Ok(Json(InternalScaleResponse {
                success: false,
                service,
                replicas: 0,
                message: Some(reason),
                reroute_to_os: Some(required_os),
            }));
        }
        Err(e) => {
            return Err(ApiError::Internal(format!("Failed to scale service: {e}")));
        }
    }

    // Get updated replica count
    let actual_replicas = manager
        .service_replica_count(&request.service)
        .await
        .unwrap_or(request.replicas as usize) as u32;

    info!(
        service = %request.service,
        replicas = actual_replicas,
        "Internal scale completed"
    );

    Ok(Json(InternalScaleResponse {
        success: true,
        service: request.service,
        replicas: actual_replicas,
        message: None,
        reroute_to_os: None,
    }))
}

/// Get the current replica count for a service.
///
/// This endpoint allows the scheduler to query the current state of a service.
///
/// # Errors
///
/// Returns an error if the service is not found or authentication is invalid.
#[allow(clippy::cast_possible_truncation)]
#[utoipa::path(
    get,
    path = "/api/v1/internal/replicas/{service}",
    params(
        ("service" = String, Path, description = "Service name"),
    ),
    responses(
        (status = 200, description = "Current replica count", body = InternalScaleResponse),
        (status = 401, description = "Unauthorized - invalid or missing internal token"),
        (status = 404, description = "Service not found"),
    ),
    tag = "Internal"
)]
pub async fn get_replicas_internal(
    _auth: InternalAuth,
    State(state): State<InternalState>,
    axum::extract::Path(service): axum::extract::Path<String>,
) -> Result<Json<InternalScaleResponse>> {
    let manager = state.service_manager.read().await;

    let replicas = manager
        .service_replica_count(&service)
        .await
        .map_err(|_| ApiError::NotFound(format!("Service '{service}' not found")))?
        as u32;

    Ok(Json(InternalScaleResponse {
        success: true,
        service,
        replicas,
        message: None,
        reroute_to_os: None,
    }))
}

/// Add a `WireGuard` peer to the local overlay transport.
///
/// Called by the cluster leader after a new node joins, so existing nodes
/// can immediately route traffic to the new peer.
///
/// `POST /api/v1/internal/add-peer`
///
/// # Errors
///
/// Returns an error if overlay networking is not configured, the endpoint
/// address is invalid, or the `WireGuard` peer cannot be added.
#[utoipa::path(
    post,
    path = "/api/v1/internal/add-peer",
    request_body = InternalAddPeerRequest,
    responses(
        (status = 200, description = "Peer added successfully", body = InternalAddPeerResponse),
        (status = 401, description = "Unauthorized - invalid or missing internal token"),
        (status = 500, description = "Internal error"),
    ),
    tag = "Internal"
)]
pub async fn add_peer_internal(
    _auth: InternalAuth,
    State(state): State<InternalState>,
    Json(request): Json<InternalAddPeerRequest>,
) -> Result<Json<InternalAddPeerResponse>> {
    let interface_name = state.overlay_interface.as_deref().ok_or_else(|| {
        ApiError::ServiceUnavailable("Overlay networking not configured on this node".into())
    })?;

    info!(
        wg_public_key = %request.wg_public_key,
        overlay_ip = %request.overlay_ip,
        endpoint = %request.endpoint,
        "Internal add-peer request received"
    );

    // Parse the endpoint into a SocketAddr
    let endpoint: std::net::SocketAddr = request.endpoint.parse().map_err(|e| {
        ApiError::BadRequest(format!(
            "Invalid endpoint address '{}': {}",
            request.endpoint, e
        ))
    })?;

    // Build a PeerInfo for the WireGuard UAPI call
    let peer_info = zlayer_overlay::PeerInfo::new(
        request.wg_public_key.clone(),
        endpoint,
        &format!("{}/32", request.overlay_ip),
        std::time::Duration::from_secs(25),
    );

    // Create a temporary OverlayTransport pointing at the existing interface's
    // UAPI socket.  We only need the interface name — the UAPI socket path is
    // derived from it (`/var/run/wireguard/{name}.sock`).
    let transport = zlayer_overlay::OverlayTransport::new(
        zlayer_overlay::OverlayConfig::default(),
        interface_name.to_string(),
    );

    transport
        .add_peer(&peer_info)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to add WireGuard peer: {e}")))?;

    info!(
        wg_public_key = %request.wg_public_key,
        overlay_ip = %request.overlay_ip,
        "Successfully added WireGuard peer via internal endpoint"
    );

    Ok(Json(InternalAddPeerResponse {
        success: true,
        message: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scale_request_deserialize() {
        let json = r#"{"service": "web", "replicas": 5}"#;
        let request: InternalScaleRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.service, "web");
        assert_eq!(request.replicas, 5);
    }

    #[test]
    fn test_scale_response_serialize() {
        let response = InternalScaleResponse {
            success: true,
            service: "web".to_string(),
            replicas: 5,
            message: None,
            reroute_to_os: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("web"));
        assert!(json.contains('5'));
        assert!(!json.contains("message")); // skip_serializing_if
    }

    #[test]
    fn test_scale_response_with_message() {
        let response = InternalScaleResponse {
            success: true,
            service: "web".to_string(),
            replicas: 5,
            message: Some("Scaled successfully".to_string()),
            reroute_to_os: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("message"));
        assert!(json.contains("Scaled successfully"));
    }

    #[test]
    fn test_add_peer_request_deserialize() {
        let json = r#"{
            "wg_public_key": "abc123base64key==",
            "overlay_ip": "10.200.0.5",
            "endpoint": "203.0.113.5:51820"
        }"#;
        let request: InternalAddPeerRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.wg_public_key, "abc123base64key==");
        assert_eq!(request.overlay_ip, "10.200.0.5");
        assert_eq!(request.endpoint, "203.0.113.5:51820");
    }

    #[test]
    fn test_add_peer_response_serialize() {
        let response = InternalAddPeerResponse {
            success: true,
            message: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("true"));
        assert!(!json.contains("message")); // skip_serializing_if
    }

    #[test]
    fn test_internal_state_with_overlay() {
        let runtime: Arc<dyn zlayer_agent::Runtime + Send + Sync> =
            Arc::new(zlayer_agent::MockRuntime::new());
        let service_manager = Arc::new(RwLock::new(ServiceManager::builder(runtime).build()));
        let state = InternalState::with_overlay(
            service_manager,
            "token".to_string(),
            Some("zl-overlay0".to_string()),
        );
        assert_eq!(state.overlay_interface.as_deref(), Some("zl-overlay0"));
    }
}
