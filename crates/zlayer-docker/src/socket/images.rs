//! Docker Engine API image endpoints.
//!
//! Stub implementations that return placeholder data.

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};

use super::types::ImageSummary;

/// Image API routes.
pub fn routes() -> Router {
    Router::new()
        .route("/images/json", get(list_images))
        .route("/images/create", post(pull_image))
        .route("/build", post(build_image))
        .route("/images/{name}/tag", post(tag_image))
        .route("/images/{name}", delete(remove_image))
        .route("/images/{name}/json", get(inspect_image))
        .route("/images/{name}/push", post(push_image))
}

/// `GET /images/json` — List images.
async fn list_images() -> Json<Vec<ImageSummary>> {
    tracing::warn!("docker API: GET /images/json — stub, returning empty list");
    Json(Vec::new())
}

/// `POST /images/create` — Pull an image.
async fn pull_image() -> impl IntoResponse {
    tracing::warn!("docker API: POST /images/create — stub");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": "image pull not yet implemented"
        })),
    )
}

/// `POST /build` — Build an image.
async fn build_image() -> impl IntoResponse {
    tracing::warn!("docker API: POST /build — stub");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": "image build not yet implemented"
        })),
    )
}

/// `POST /images/{name}/tag` — Tag an image.
async fn tag_image(Path(name): Path<String>) -> impl IntoResponse {
    tracing::warn!("docker API: POST /images/{name}/tag — stub");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": format!("image tag not yet implemented for {name}")
        })),
    )
}

/// `DELETE /images/{name}` — Remove an image.
async fn remove_image(Path(name): Path<String>) -> impl IntoResponse {
    tracing::warn!("docker API: DELETE /images/{name} — stub");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": format!("image remove not yet implemented for {name}")
        })),
    )
}

/// `GET /images/{name}/json` — Inspect an image.
async fn inspect_image(Path(name): Path<String>) -> impl IntoResponse {
    tracing::warn!("docker API: GET /images/{name}/json — stub");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": format!("image inspect not yet implemented for {name}")
        })),
    )
}

/// `POST /images/{name}/push` — Push an image.
async fn push_image(Path(name): Path<String>) -> impl IntoResponse {
    tracing::warn!("docker API: POST /images/{name}/push — stub");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": format!("image push not yet implemented for {name}")
        })),
    )
}
