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

/// Query parameters for [`remove_image_handler`].
#[derive(Debug, Deserialize, IntoParams)]
pub struct RemoveImageQuery {
    /// Force removal even if the image is referenced by containers.
    #[serde(default)]
    pub force: bool,
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

/// Build the image-management routes.
pub fn image_routes() -> axum::Router<ImageState> {
    use axum::routing::{delete, get, post};
    axum::Router::new()
        .route("/images", get(list_images_handler))
        .route("/images/{image}", delete(remove_image_handler))
        .route("/system/prune", post(prune_images_handler))
}
