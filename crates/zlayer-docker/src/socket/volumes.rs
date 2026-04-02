//! Docker Engine API volume endpoints.
//!
//! Stub implementations that return placeholder data.

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};

use super::types::VolumeListResponse;

/// Volume API routes.
pub fn routes() -> Router {
    Router::new()
        .route("/volumes", get(list_volumes))
        .route("/volumes/create", post(create_volume))
        .route("/volumes/{name}", delete(remove_volume))
        .route("/volumes/prune", post(prune_volumes))
}

/// `GET /volumes` — List volumes.
async fn list_volumes() -> Json<VolumeListResponse> {
    tracing::warn!("docker API: GET /volumes — stub, returning empty list");
    Json(VolumeListResponse {
        volumes: Vec::new(),
        warnings: Vec::new(),
    })
}

/// `POST /volumes/create` — Create a volume.
async fn create_volume() -> impl IntoResponse {
    tracing::warn!("docker API: POST /volumes/create — stub");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": "volume create not yet implemented"
        })),
    )
}

/// `DELETE /volumes/{name}` — Remove a volume.
async fn remove_volume(Path(name): Path<String>) -> impl IntoResponse {
    tracing::warn!("docker API: DELETE /volumes/{name} — stub");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": format!("volume remove not yet implemented for {name}")
        })),
    )
}

/// `POST /volumes/prune` — Prune unused volumes.
async fn prune_volumes() -> impl IntoResponse {
    tracing::warn!("docker API: POST /volumes/prune — stub");
    Json(serde_json::json!({
        "VolumesDeleted": [],
        "SpaceReclaimed": 0
    }))
}
