//! Build API endpoints
//!
//! Provides REST API endpoints for building container images using the builder crate.
//! Supports:
//! - Starting builds from Dockerfile or runtime templates
//! - Streaming build progress via SSE
//! - Querying build status and logs
//! - Listing available runtime templates

use std::collections::HashMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    extract::{Multipart, Path, State},
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
    routing::{get, post},
    Json, Router,
};
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use tracing::{debug, error, info, warn};
use utoipa::ToSchema;
use uuid::Uuid;

use zlayer_builder::{list_templates, BuildEvent, ImageBuilder, Runtime, RuntimeInfo};

use crate::auth::AuthUser;
use crate::error::{ApiError, Result};

// ============================================================================
// State Types
// ============================================================================

/// Shared state for build management
#[derive(Clone)]
pub struct BuildState {
    /// Build manager for tracking builds
    pub manager: Arc<BuildManager>,
}

impl BuildState {
    /// Create a new build state with the given build directory
    pub fn new(build_dir: PathBuf) -> Self {
        Self {
            manager: Arc::new(BuildManager::new(build_dir)),
        }
    }
}

/// Build manager - tracks active builds and their state
pub struct BuildManager {
    /// Directory for storing build contexts and logs
    build_dir: PathBuf,
    /// Active builds indexed by ID
    builds: RwLock<HashMap<String, BuildStatus>>,
    /// Event channels for streaming build progress
    event_channels: RwLock<HashMap<String, broadcast::Sender<BuildEventWrapper>>>,
}

impl BuildManager {
    /// Create a new build manager
    pub fn new(build_dir: PathBuf) -> Self {
        Self {
            build_dir,
            builds: RwLock::new(HashMap::new()),
            event_channels: RwLock::new(HashMap::new()),
        }
    }

    /// Get the build directory
    pub fn build_dir(&self) -> &PathBuf {
        &self.build_dir
    }

    /// Register a new build
    pub async fn register_build(&self, id: String) -> broadcast::Sender<BuildEventWrapper> {
        let (tx, _) = broadcast::channel(256);

        let status = BuildStatus {
            id: id.clone(),
            status: BuildStateEnum::Pending,
            image_id: None,
            error: None,
            started_at: chrono::Utc::now().to_rfc3339(),
            completed_at: None,
        };

        {
            let mut builds = self.builds.write().await;
            builds.insert(id.clone(), status);
        }

        {
            let mut channels = self.event_channels.write().await;
            channels.insert(id, tx.clone());
        }

        tx
    }

    /// Get build status
    pub async fn get_status(&self, id: &str) -> Option<BuildStatus> {
        let builds = self.builds.read().await;
        builds.get(id).cloned()
    }

    /// Update build status
    pub async fn update_status(
        &self,
        id: &str,
        status: BuildStateEnum,
        image_id: Option<String>,
        error: Option<String>,
    ) {
        let mut builds = self.builds.write().await;
        if let Some(build) = builds.get_mut(id) {
            build.status = status;
            build.image_id = image_id;
            build.error = error;
            if matches!(status, BuildStateEnum::Complete | BuildStateEnum::Failed) {
                build.completed_at = Some(chrono::Utc::now().to_rfc3339());
            }
        }
    }

    /// Subscribe to build events
    pub async fn subscribe(&self, id: &str) -> Option<broadcast::Receiver<BuildEventWrapper>> {
        let channels = self.event_channels.read().await;
        channels.get(id).map(|tx| tx.subscribe())
    }

    /// Clean up build resources
    pub async fn cleanup_build(&self, id: &str) {
        // Remove event channel (keeps build status for history)
        let mut channels = self.event_channels.write().await;
        channels.remove(id);
    }

    /// List all builds
    pub async fn list_builds(&self, limit: usize) -> Vec<BuildStatus> {
        let builds = self.builds.read().await;
        builds.values().take(limit).cloned().collect()
    }
}

// ============================================================================
// Request/Response Types
// ============================================================================

/// Build request for JSON API
#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct BuildRequest {
    /// Use runtime template instead of Dockerfile
    #[serde(default)]
    pub runtime: Option<String>,
    /// Build arguments (ARG values)
    #[serde(default)]
    pub build_args: HashMap<String, String>,
    /// Target stage for multi-stage builds
    #[serde(default)]
    pub target: Option<String>,
    /// Tags to apply to the image
    #[serde(default)]
    pub tags: Vec<String>,
    /// Disable cache
    #[serde(default)]
    pub no_cache: bool,
    /// Push to registry after build
    #[serde(default)]
    pub push: bool,
}

/// Build status response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
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

/// Build state enumeration
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
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

/// Runtime template information
#[derive(Debug, Serialize, ToSchema)]
pub struct TemplateInfo {
    /// Template name (e.g., "node20")
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// Files that indicate this runtime should be used
    pub detect_files: Vec<String>,
}

impl From<&RuntimeInfo> for TemplateInfo {
    fn from(info: &RuntimeInfo) -> Self {
        Self {
            name: info.name.to_string(),
            description: info.description.to_string(),
            detect_files: info.detect_files.iter().map(|s| s.to_string()).collect(),
        }
    }
}

/// Trigger build response
#[derive(Debug, Serialize, ToSchema)]
pub struct TriggerBuildResponse {
    /// Unique build ID for tracking
    pub build_id: String,
    /// Human-readable message
    pub message: String,
}

/// Build event wrapper for SSE serialization
#[derive(Debug, Clone, Serialize)]
pub struct BuildEventWrapper {
    /// Event type
    #[serde(rename = "type")]
    pub event_type: String,
    /// Event data
    pub data: serde_json::Value,
}

impl From<BuildEvent> for BuildEventWrapper {
    fn from(event: BuildEvent) -> Self {
        match event {
            BuildEvent::StageStarted {
                index,
                name,
                base_image,
            } => BuildEventWrapper {
                event_type: "stage_started".to_string(),
                data: serde_json::json!({
                    "index": index,
                    "name": name,
                    "base_image": base_image,
                }),
            },
            BuildEvent::InstructionStarted {
                stage,
                index,
                instruction,
            } => BuildEventWrapper {
                event_type: "instruction_started".to_string(),
                data: serde_json::json!({
                    "stage": stage,
                    "index": index,
                    "instruction": instruction,
                }),
            },
            BuildEvent::Output { line, is_stderr } => BuildEventWrapper {
                event_type: "output".to_string(),
                data: serde_json::json!({
                    "line": line,
                    "is_stderr": is_stderr,
                }),
            },
            BuildEvent::InstructionComplete {
                stage,
                index,
                cached,
            } => BuildEventWrapper {
                event_type: "instruction_complete".to_string(),
                data: serde_json::json!({
                    "stage": stage,
                    "index": index,
                    "cached": cached,
                }),
            },
            BuildEvent::StageComplete { index } => BuildEventWrapper {
                event_type: "stage_complete".to_string(),
                data: serde_json::json!({
                    "index": index,
                }),
            },
            BuildEvent::BuildComplete { image_id } => BuildEventWrapper {
                event_type: "build_complete".to_string(),
                data: serde_json::json!({
                    "image_id": image_id,
                }),
            },
            BuildEvent::BuildFailed { error } => BuildEventWrapper {
                event_type: "build_failed".to_string(),
                data: serde_json::json!({
                    "error": error,
                }),
            },
        }
    }
}

// ============================================================================
// Handlers
// ============================================================================

/// POST /api/v1/build
/// Start a new build from multipart upload (Dockerfile + context tarball)
///
/// Accepts a multipart form with:
/// - `dockerfile`: The Dockerfile content (optional if using runtime)
/// - `context`: A tarball containing the build context
/// - `config`: JSON configuration (BuildRequest)
#[utoipa::path(
    post,
    path = "/api/v1/build",
    request_body(content_type = "multipart/form-data", description = "Build context with Dockerfile"),
    responses(
        (status = 202, description = "Build started", body = TriggerBuildResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal error"),
    ),
    security(("bearer_auth" = [])),
    tag = "Build"
)]
pub async fn start_build(
    _user: AuthUser,
    State(state): State<BuildState>,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<TriggerBuildResponse>)> {
    let build_id = Uuid::new_v4().to_string();
    let context_dir = state.manager.build_dir().join(&build_id);

    // Create build context directory
    tokio::fs::create_dir_all(&context_dir).await.map_err(|e| {
        error!("Failed to create build context directory: {}", e);
        ApiError::Internal(format!("Failed to create build context: {}", e))
    })?;

    let mut config: Option<BuildRequest> = None;
    let mut has_dockerfile = false;

    // Process multipart fields
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("Failed to read multipart field: {}", e)))?
    {
        let name = field.name().unwrap_or("").to_string();
        let data = field
            .bytes()
            .await
            .map_err(|e| ApiError::BadRequest(format!("Failed to read field data: {}", e)))?;

        match name.as_str() {
            "dockerfile" => {
                tokio::fs::write(context_dir.join("Dockerfile"), &data)
                    .await
                    .map_err(|e| {
                        ApiError::Internal(format!("Failed to write Dockerfile: {}", e))
                    })?;
                has_dockerfile = true;
                debug!("Wrote Dockerfile for build {}", build_id);
            }
            "context" => {
                // Extract tarball to context directory
                extract_tarball(&data, &context_dir).await?;
                debug!("Extracted context tarball for build {}", build_id);
            }
            "config" => {
                let config_str = std::str::from_utf8(&data)
                    .map_err(|e| ApiError::BadRequest(format!("Invalid config encoding: {}", e)))?;
                config =
                    Some(serde_json::from_str(config_str).map_err(|e| {
                        ApiError::BadRequest(format!("Invalid config JSON: {}", e))
                    })?);
            }
            _ => {
                warn!("Ignoring unknown multipart field: {}", name);
            }
        }
    }

    let request = config.unwrap_or_default();

    // Validate: must have dockerfile OR runtime template
    if !has_dockerfile && request.runtime.is_none() {
        // Check if Dockerfile was included in the tarball
        if !context_dir.join("Dockerfile").exists() {
            return Err(ApiError::BadRequest(
                "Either a Dockerfile or runtime template must be provided".to_string(),
            ));
        }
    }

    // Register build and start it
    let event_tx = state.manager.register_build(build_id.clone()).await;

    spawn_build(
        state.manager.clone(),
        build_id.clone(),
        context_dir,
        request,
        event_tx,
    );

    info!("Started build {}", build_id);

    Ok((
        StatusCode::ACCEPTED,
        Json(TriggerBuildResponse {
            build_id: build_id.clone(),
            message: format!("Build {} started", build_id),
        }),
    ))
}

/// POST /api/v1/build/json
/// Start a new build from JSON request with a context path on the server
#[utoipa::path(
    post,
    path = "/api/v1/build/json",
    request_body = BuildRequestWithContext,
    responses(
        (status = 202, description = "Build started", body = TriggerBuildResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal error"),
    ),
    security(("bearer_auth" = [])),
    tag = "Build"
)]
pub async fn start_build_json(
    _user: AuthUser,
    State(state): State<BuildState>,
    Json(request): Json<BuildRequestWithContext>,
) -> Result<(StatusCode, Json<TriggerBuildResponse>)> {
    let build_id = Uuid::new_v4().to_string();

    // Validate context path exists
    let context_path_str = request.context_path.clone();
    let context_path = PathBuf::from(&context_path_str);
    if !context_path.exists() {
        return Err(ApiError::BadRequest(format!(
            "Context path does not exist: {}",
            context_path_str
        )));
    }

    // Register build and start it
    let event_tx = state.manager.register_build(build_id.clone()).await;

    spawn_build(
        state.manager.clone(),
        build_id.clone(),
        context_path,
        request.into(),
        event_tx,
    );

    info!(
        "Started build {} from context {}",
        build_id, context_path_str
    );

    Ok((
        StatusCode::ACCEPTED,
        Json(TriggerBuildResponse {
            build_id: build_id.clone(),
            message: format!("Build {} started", build_id),
        }),
    ))
}

/// Build request with server-side context path
#[derive(Debug, Deserialize, ToSchema)]
pub struct BuildRequestWithContext {
    /// Path to the build context on the server
    pub context_path: String,
    /// Use runtime template instead of Dockerfile
    #[serde(default)]
    pub runtime: Option<String>,
    /// Build arguments
    #[serde(default)]
    pub build_args: HashMap<String, String>,
    /// Target stage
    #[serde(default)]
    pub target: Option<String>,
    /// Tags to apply
    #[serde(default)]
    pub tags: Vec<String>,
    /// Disable cache
    #[serde(default)]
    pub no_cache: bool,
    /// Push after build
    #[serde(default)]
    pub push: bool,
}

impl From<BuildRequestWithContext> for BuildRequest {
    fn from(req: BuildRequestWithContext) -> Self {
        Self {
            runtime: req.runtime,
            build_args: req.build_args,
            target: req.target,
            tags: req.tags,
            no_cache: req.no_cache,
            push: req.push,
        }
    }
}

/// GET /api/v1/build/{id}
/// Get build status
#[utoipa::path(
    get,
    path = "/api/v1/build/{id}",
    params(
        ("id" = String, Path, description = "Build ID"),
    ),
    responses(
        (status = 200, description = "Build status", body = BuildStatus),
        (status = 404, description = "Build not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Build"
)]
pub async fn get_build_status(
    _user: AuthUser,
    State(state): State<BuildState>,
    Path(build_id): Path<String>,
) -> Result<Json<BuildStatus>> {
    let status = state
        .manager
        .get_status(&build_id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Build '{}' not found", build_id)))?;

    Ok(Json(status))
}

/// GET /api/v1/build/{id}/stream
/// Stream build progress via Server-Sent Events
#[utoipa::path(
    get,
    path = "/api/v1/build/{id}/stream",
    params(
        ("id" = String, Path, description = "Build ID"),
    ),
    responses(
        (status = 200, description = "SSE event stream"),
        (status = 404, description = "Build not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Build"
)]
pub async fn stream_build(
    _user: AuthUser,
    State(state): State<BuildState>,
    Path(build_id): Path<String>,
) -> Result<Sse<impl Stream<Item = std::result::Result<Event, Infallible>>>> {
    let rx = state.manager.subscribe(&build_id).await.ok_or_else(|| {
        ApiError::NotFound(format!("Build '{}' not found or not active", build_id))
    })?;

    let stream = BroadcastStream::new(rx).map(|result| {
        let event = match result {
            Ok(wrapper) => wrapper,
            Err(_) => BuildEventWrapper {
                event_type: "error".to_string(),
                data: serde_json::json!({"message": "Stream error"}),
            },
        };

        Ok(Event::default()
            .event(&event.event_type)
            .json_data(&event.data)
            .unwrap_or_else(|_| Event::default().data("error")))
    });

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// GET /api/v1/build/{id}/logs
/// Get build logs
#[utoipa::path(
    get,
    path = "/api/v1/build/{id}/logs",
    params(
        ("id" = String, Path, description = "Build ID"),
    ),
    responses(
        (status = 200, description = "Build logs", body = String),
        (status = 404, description = "Build not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Build"
)]
pub async fn get_build_logs(
    _user: AuthUser,
    State(state): State<BuildState>,
    Path(build_id): Path<String>,
) -> Result<String> {
    // Check if build exists
    let _status = state
        .manager
        .get_status(&build_id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Build '{}' not found", build_id)))?;

    let log_path = state.manager.build_dir().join(&build_id).join("build.log");

    match tokio::fs::read_to_string(&log_path).await {
        Ok(logs) => Ok(logs),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(String::new()) // No logs yet
        }
        Err(e) => Err(ApiError::Internal(format!("Failed to read logs: {}", e))),
    }
}

/// GET /api/v1/builds
/// List all builds
#[utoipa::path(
    get,
    path = "/api/v1/builds",
    responses(
        (status = 200, description = "List of builds", body = Vec<BuildStatus>),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Build"
)]
pub async fn list_builds(
    _user: AuthUser,
    State(state): State<BuildState>,
) -> Result<Json<Vec<BuildStatus>>> {
    let builds = state.manager.list_builds(100).await;
    Ok(Json(builds))
}

/// GET /api/v1/templates
/// List available runtime templates
#[utoipa::path(
    get,
    path = "/api/v1/templates",
    responses(
        (status = 200, description = "List of templates", body = Vec<TemplateInfo>),
    ),
    tag = "Build"
)]
pub async fn list_runtime_templates() -> Json<Vec<TemplateInfo>> {
    let templates: Vec<TemplateInfo> = list_templates()
        .into_iter()
        .map(|info| info.into())
        .collect();

    Json(templates)
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Extract a tarball to a directory
async fn extract_tarball(data: &[u8], target_dir: &std::path::Path) -> Result<()> {
    use std::io::Cursor;

    // Copy data to owned buffer for 'static lifetime in spawn_blocking
    let data_owned = data.to_vec();
    let target = target_dir.to_path_buf();

    // Extract in a blocking task since tar operations are synchronous
    tokio::task::spawn_blocking(move || {
        let cursor = Cursor::new(data_owned);
        let gz = flate2::read::GzDecoder::new(cursor);
        let mut archive = tar::Archive::new(gz);
        archive.unpack(&target)
    })
    .await
    .map_err(|e| ApiError::Internal(format!("Task join error: {}", e)))?
    .map_err(|e| ApiError::BadRequest(format!("Failed to extract tarball: {}", e)))?;

    Ok(())
}

/// Spawn the build process in a background task
fn spawn_build(
    manager: Arc<BuildManager>,
    build_id: String,
    context_dir: PathBuf,
    request: BuildRequest,
    event_tx: broadcast::Sender<BuildEventWrapper>,
) {
    tokio::spawn(async move {
        // Update status to running
        manager
            .update_status(&build_id, BuildStateEnum::Running, None, None)
            .await;

        // Create log file
        let log_path = context_dir.join("build.log");
        let log_file = match tokio::fs::File::create(&log_path).await {
            Ok(f) => Some(Arc::new(tokio::sync::Mutex::new(f))),
            Err(e) => {
                warn!("Failed to create log file: {}", e);
                None
            }
        };

        // Build the image
        let result = run_build(&context_dir, &request, &event_tx, log_file.clone()).await;

        match result {
            Ok(image_id) => {
                info!("Build {} completed: {}", build_id, image_id);
                manager
                    .update_status(
                        &build_id,
                        BuildStateEnum::Complete,
                        Some(image_id.clone()),
                        None,
                    )
                    .await;

                let _ = event_tx.send(BuildEventWrapper {
                    event_type: "build_complete".to_string(),
                    data: serde_json::json!({"image_id": image_id}),
                });
            }
            Err(e) => {
                error!("Build {} failed: {}", build_id, e);
                let error_msg = e.to_string();
                manager
                    .update_status(
                        &build_id,
                        BuildStateEnum::Failed,
                        None,
                        Some(error_msg.clone()),
                    )
                    .await;

                let _ = event_tx.send(BuildEventWrapper {
                    event_type: "build_failed".to_string(),
                    data: serde_json::json!({"error": error_msg}),
                });
            }
        }

        // Cleanup event channel (keep build status for history)
        manager.cleanup_build(&build_id).await;
    });
}

/// Run the actual build process
async fn run_build(
    context_dir: &PathBuf,
    request: &BuildRequest,
    event_tx: &broadcast::Sender<BuildEventWrapper>,
    log_file: Option<Arc<tokio::sync::Mutex<tokio::fs::File>>>,
) -> std::result::Result<String, zlayer_builder::BuildError> {
    // Create event channel for the builder
    let (builder_tx, builder_rx) = std::sync::mpsc::channel::<BuildEvent>();

    // Forward events to SSE and log file
    let event_tx_clone = event_tx.clone();
    let log_file_clone = log_file.clone();
    let event_forwarder = tokio::task::spawn_blocking(move || {
        while let Ok(event) = builder_rx.recv() {
            // Forward to SSE
            let wrapper = BuildEventWrapper::from(event.clone());
            let _ = event_tx_clone.send(wrapper);

            // Log output lines
            if let BuildEvent::Output { ref line, .. } = event {
                if let Some(ref log) = log_file_clone {
                    // Best effort logging
                    let log_clone = log.clone();
                    let line_clone = line.clone();
                    tokio::spawn(async move {
                        use tokio::io::AsyncWriteExt;
                        let mut file = log_clone.lock().await;
                        let _ = file.write_all(line_clone.as_bytes()).await;
                        let _ = file.write_all(b"\n").await;
                    });
                }
            }
        }
    });

    // Create image builder
    let mut builder = ImageBuilder::new(context_dir).await?;

    // Apply runtime template if specified
    if let Some(ref runtime_name) = request.runtime {
        let runtime = Runtime::from_name(runtime_name).ok_or_else(|| {
            zlayer_builder::BuildError::InvalidInstruction {
                instruction: "runtime".to_string(),
                reason: format!("Unknown runtime: {}", runtime_name),
            }
        })?;
        builder = builder.runtime(runtime);
    }

    // Apply build arguments
    for (key, value) in &request.build_args {
        builder = builder.build_arg(key, value);
    }

    // Apply target stage
    if let Some(ref target) = request.target {
        builder = builder.target(target);
    }

    // Apply tags
    for tag in &request.tags {
        builder = builder.tag(tag);
    }

    // Apply no-cache
    if request.no_cache {
        builder = builder.no_cache();
    }

    // Apply push
    if request.push {
        builder = builder.push_without_auth();
    }

    // Set event sender
    builder = builder.with_events(builder_tx);

    // Run the build
    let result = builder.build().await?;

    // Wait for event forwarder to finish
    let _ = event_forwarder.await;

    Ok(result.image_id)
}

// ============================================================================
// Router
// ============================================================================

/// Create router for build endpoints
pub fn build_routes() -> Router<BuildState> {
    Router::new()
        .route("/build", post(start_build))
        .route("/build/json", post(start_build_json))
        .route("/build/{id}", get(get_build_status))
        .route("/build/{id}/stream", get(stream_build))
        .route("/build/{id}/logs", get(get_build_logs))
        .route("/builds", get(list_builds))
        .route("/templates", get(list_runtime_templates))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_state_enum_serialize() {
        assert_eq!(
            serde_json::to_string(&BuildStateEnum::Pending).unwrap(),
            "\"pending\""
        );
        assert_eq!(
            serde_json::to_string(&BuildStateEnum::Running).unwrap(),
            "\"running\""
        );
        assert_eq!(
            serde_json::to_string(&BuildStateEnum::Complete).unwrap(),
            "\"complete\""
        );
        assert_eq!(
            serde_json::to_string(&BuildStateEnum::Failed).unwrap(),
            "\"failed\""
        );
    }

    #[test]
    fn test_build_request_default() {
        let req = BuildRequest::default();
        assert!(req.runtime.is_none());
        assert!(req.build_args.is_empty());
        assert!(req.target.is_none());
        assert!(req.tags.is_empty());
        assert!(!req.no_cache);
        assert!(!req.push);
    }

    #[test]
    fn test_build_event_wrapper_from_build_event() {
        let event = BuildEvent::StageStarted {
            index: 0,
            name: Some("builder".to_string()),
            base_image: "node:20".to_string(),
        };
        let wrapper = BuildEventWrapper::from(event);
        assert_eq!(wrapper.event_type, "stage_started");
        assert_eq!(wrapper.data["index"], 0);
        assert_eq!(wrapper.data["name"], "builder");
        assert_eq!(wrapper.data["base_image"], "node:20");
    }

    #[test]
    fn test_template_info_from_runtime_info() {
        let templates = list_templates();
        let first = templates.first().unwrap();
        let info: TemplateInfo = (*first).into();
        assert!(!info.name.is_empty());
        assert!(!info.description.is_empty());
    }

    #[test]
    fn test_build_status_serialize() {
        let status = BuildStatus {
            id: "test-123".to_string(),
            status: BuildStateEnum::Complete,
            image_id: Some("sha256:abc".to_string()),
            error: None,
            started_at: "2025-01-26T12:00:00Z".to_string(),
            completed_at: Some("2025-01-26T12:05:00Z".to_string()),
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("test-123"));
        assert!(json.contains("complete"));
        assert!(json.contains("sha256:abc"));
        // Error should be skipped (None)
        assert!(!json.contains("error"));
    }

    #[tokio::test]
    async fn test_build_manager() {
        let temp_dir = tempfile::tempdir().unwrap();
        let manager = BuildManager::new(temp_dir.path().to_path_buf());

        // Register a build
        let _tx = manager.register_build("test-1".to_string()).await;

        // Check status
        let status = manager.get_status("test-1").await.unwrap();
        assert_eq!(status.id, "test-1");
        assert_eq!(status.status, BuildStateEnum::Pending);

        // Update status
        manager
            .update_status(
                "test-1",
                BuildStateEnum::Complete,
                Some("sha256:abc".to_string()),
                None,
            )
            .await;

        let status = manager.get_status("test-1").await.unwrap();
        assert_eq!(status.status, BuildStateEnum::Complete);
        assert_eq!(status.image_id, Some("sha256:abc".to_string()));
        assert!(status.completed_at.is_some());
    }
}
