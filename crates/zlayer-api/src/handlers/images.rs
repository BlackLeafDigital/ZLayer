//! Image management endpoints (list / remove / prune)
//!
//! Exposes the `Runtime` trait's image-management methods over HTTP so the
//! `zlayer image ls`, `zlayer image rm`, and `zlayer system prune` CLI
//! subcommands can operate against a remote daemon.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::auth::AuthUser;
use crate::error::{ApiError, Result};
use zlayer_agent::runtime::{ImageInfo, PruneResult, Runtime};
use zlayer_spec::PullPolicy;

/// State for image-management endpoints.
///
/// Holds an owned handle to the runtime so handlers can dispatch to
/// `list_images`, `remove_image`, and `prune_images`.
#[derive(Clone)]
pub struct ImageState {
    /// Container runtime (Youki / Docker / WASM / mock depending on daemon).
    pub runtime: Arc<dyn Runtime + Send + Sync>,
}

impl ImageState {
    /// Create a new image state from a runtime handle.
    #[must_use]
    pub fn new(runtime: Arc<dyn Runtime + Send + Sync>) -> Self {
        Self { runtime }
    }
}

/// Serializable wrapper for [`ImageInfo`] so we can attach `ToSchema` here
/// (the underlying type in `zlayer-agent` can't depend on `utoipa`).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ImageInfoDto {
    /// Canonical image reference (e.g. `zachhandley/zlayer-manager:latest`).
    pub reference: String,
    /// Content-addressed digest (`sha256:...`) if known.
    pub digest: Option<String>,
    /// Size in bytes if known.
    pub size_bytes: Option<u64>,
}

impl From<ImageInfo> for ImageInfoDto {
    fn from(info: ImageInfo) -> Self {
        Self {
            reference: info.reference,
            digest: info.digest,
            size_bytes: info.size_bytes,
        }
    }
}

/// Serializable wrapper for [`PruneResult`].
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
pub struct PruneResultDto {
    /// Image references or digests that were removed.
    pub deleted: Vec<String>,
    /// Bytes reclaimed from the cache.
    pub space_reclaimed: u64,
}

impl From<PruneResult> for PruneResultDto {
    fn from(result: PruneResult) -> Self {
        Self {
            deleted: result.deleted,
            space_reclaimed: result.space_reclaimed,
        }
    }
}

/// Request body for [`pull_image_handler`]. Blocking pull of an OCI image.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct PullImageRequest {
    /// OCI image reference to pull, e.g. `docker.io/library/nginx:latest`.
    pub reference: String,
    /// Pull policy override. Accepts `"always"`, `"if_not_present"`, or
    /// `"never"`. Defaults to `"always"` when omitted.
    #[serde(default)]
    pub pull_policy: Option<String>,
}

/// Response body for [`pull_image_handler`]. Reports the pulled reference
/// and, when the backend exposes it via `list_images`, the resolved digest
/// and on-disk size.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct PullImageResponse {
    /// Canonical reference that was pulled.
    pub reference: String,
    /// Content-addressed digest (`sha256:...`) if the runtime reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// On-disk size in bytes if the runtime reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

/// Query parameters for [`remove_image_handler`].
#[derive(Debug, Deserialize, IntoParams)]
pub struct RemoveImageQuery {
    /// Force removal even if the image is referenced by containers.
    #[serde(default)]
    pub force: bool,
}

/// Request body for [`tag_image_handler`]. Matches Docker-compat
/// `docker tag` semantics: create a new reference (`target`) pointing at an
/// already-cached image (`source`).
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct TagImageRequest {
    /// Existing image reference to tag (e.g. `myapp:latest`).
    pub source: String,
    /// New reference to create (e.g. `registry.example.com/myapp:v1`).
    pub target: String,
}

/// List all cached images known to the runtime.
///
/// # Errors
///
/// Returns an error if authentication fails or the runtime cannot enumerate
/// its image cache (for example, when the backend does not implement
/// `list_images`).
#[utoipa::path(
    get,
    path = "/api/v1/images",
    responses(
        (status = 200, description = "List of cached images", body = Vec<ImageInfoDto>),
        (status = 401, description = "Unauthorized"),
        (status = 501, description = "Runtime does not support image listing"),
    ),
    security(("bearer_auth" = [])),
    tag = "Images"
)]
pub async fn list_images_handler(
    State(state): State<ImageState>,
    _auth: AuthUser,
) -> Result<Json<Vec<ImageInfoDto>>> {
    let images = state
        .runtime
        .list_images()
        .await
        .map_err(|e| ApiError::Internal(format!("failed to list images: {e}")))?;
    Ok(Json(images.into_iter().map(ImageInfoDto::from).collect()))
}

/// Remove an image from the runtime's cache.
///
/// # Errors
///
/// Returns an error if authentication fails, the image cannot be found, or
/// the runtime backend does not support image removal.
#[utoipa::path(
    delete,
    path = "/api/v1/images/{image}",
    params(
        ("image" = String, Path, description = "Image reference (URL-encoded)"),
        RemoveImageQuery,
    ),
    responses(
        (status = 204, description = "Image removed"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Image not found"),
        (status = 501, description = "Runtime does not support image removal"),
    ),
    security(("bearer_auth" = [])),
    tag = "Images"
)]
pub async fn remove_image_handler(
    State(state): State<ImageState>,
    _auth: AuthUser,
    Path(image): Path<String>,
    Query(q): Query<RemoveImageQuery>,
) -> Result<StatusCode> {
    state
        .runtime
        .remove_image(&image, q.force)
        .await
        .map_err(|e| ApiError::Internal(format!("failed to remove image: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Prune dangling / unused images from the runtime's cache.
///
/// # Errors
///
/// Returns an error if authentication fails or the runtime backend does not
/// support pruning.
#[utoipa::path(
    post,
    path = "/api/v1/system/prune",
    responses(
        (status = 200, description = "Prune result", body = PruneResultDto),
        (status = 401, description = "Unauthorized"),
        (status = 501, description = "Runtime does not support pruning"),
    ),
    security(("bearer_auth" = [])),
    tag = "Images"
)]
pub async fn prune_images_handler(
    State(state): State<ImageState>,
    _auth: AuthUser,
) -> Result<Json<PruneResultDto>> {
    let result = state
        .runtime
        .prune_images()
        .await
        .map_err(|e| ApiError::Internal(format!("failed to prune images: {e}")))?;
    Ok(Json(PruneResultDto::from(result)))
}

/// Pull an OCI image into the runtime's local cache.
///
/// This is a blocking pull: the handler returns only after the image is
/// resolved and stored locally (or the pull fails). When `pull_policy` is
/// omitted the default is `"always"`, matching Docker-compat semantics for
/// `POST /images/create`. On success, the response echoes the reference and
/// best-effort `digest`/`size_bytes` resolved via `list_images()`.
///
/// # Errors
///
/// Returns an error if authentication fails, the reference is empty, the
/// image cannot be pulled, or the user lacks the operator role.
#[utoipa::path(
    post,
    path = "/api/v1/images/pull",
    request_body = PullImageRequest,
    responses(
        (status = 200, description = "Image pulled", body = PullImageResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - operator role required"),
        (status = 500, description = "Pull failed"),
    ),
    security(("bearer_auth" = [])),
    tag = "Images"
)]
pub async fn pull_image_handler(
    State(state): State<ImageState>,
    user: AuthUser,
    Json(request): Json<PullImageRequest>,
) -> Result<Json<PullImageResponse>> {
    user.require_role("operator")?;

    if request.reference.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "reference is required and cannot be empty".to_string(),
        ));
    }

    let policy = match request.pull_policy.as_deref() {
        Some("never") => PullPolicy::Never,
        Some("if_not_present") => PullPolicy::IfNotPresent,
        _ => PullPolicy::Always,
    };

    state
        .runtime
        .pull_image_with_policy(&request.reference, policy)
        .await
        .map_err(|e| ApiError::Internal(format!("failed to pull image: {e}")))?;

    // Best-effort: look up digest/size from the image cache. If the runtime
    // doesn't support `list_images`, we return just the reference.
    let (digest, size_bytes) = match state.runtime.list_images().await {
        Ok(images) => images
            .into_iter()
            .find(|info| info.reference == request.reference)
            .map_or((None, None), |info| (info.digest, info.size_bytes)),
        Err(_) => (None, None),
    };

    Ok(Json(PullImageResponse {
        reference: request.reference,
        digest,
        size_bytes,
    }))
}

/// Create a new tag pointing at an existing image.
///
/// Docker-compat `POST /api/v1/images/tag`: takes `{ source, target }` and
/// asks the runtime to make `target` resolve to the same content as `source`.
/// Both references must be non-empty; `target` is split on the last `:` into
/// repository + tag (defaulting tag to `latest`).
///
/// # Errors
///
/// Returns `400` if the request body is malformed, `404` if the source image
/// is not in the cache, `403` if the caller lacks the `operator` role, `501`
/// if the runtime does not support tagging, and `500` for other runtime
/// errors.
#[utoipa::path(
    post,
    path = "/api/v1/images/tag",
    request_body = TagImageRequest,
    responses(
        (status = 204, description = "Tag created"),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - operator role required"),
        (status = 404, description = "Source image not found"),
        (status = 501, description = "Runtime does not support tagging"),
        (status = 500, description = "Internal error"),
    ),
    security(("bearer_auth" = [])),
    tag = "Images"
)]
pub async fn tag_image_handler(
    State(state): State<ImageState>,
    user: AuthUser,
    Json(request): Json<TagImageRequest>,
) -> Result<StatusCode> {
    user.require_role("operator")?;

    if request.source.trim().is_empty() || request.target.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "source and target must be non-empty image references".to_string(),
        ));
    }

    state
        .runtime
        .tag_image(&request.source, &request.target)
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { reason, .. } => {
                ApiError::NotFound(format!("Source image not found: {reason}"))
            }
            zlayer_agent::AgentError::InvalidSpec(reason) => ApiError::BadRequest(reason),
            zlayer_agent::AgentError::Unsupported(reason) => {
                ApiError::Internal(format!("Runtime does not support tagging: {reason}"))
            }
            other => ApiError::Internal(format!("Failed to tag image: {other}")),
        })?;

    Ok(StatusCode::NO_CONTENT)
}

/// Build the image-management routes.
pub fn image_routes() -> axum::Router<ImageState> {
    use axum::routing::{delete, get, post};
    axum::Router::new()
        .route("/images", get(list_images_handler))
        .route("/images/pull", post(pull_image_handler))
        .route("/images/tag", post(tag_image_handler))
        .route("/images/{image}", delete(remove_image_handler))
        .route("/system/prune", post(prune_images_handler))
}
