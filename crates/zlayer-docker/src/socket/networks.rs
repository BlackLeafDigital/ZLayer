//! Docker Engine API network endpoints.
//!
//! Stub implementations that return placeholder data.

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};

use super::types::NetworkSummary;

/// Network API routes.
pub fn routes() -> Router {
    Router::new()
        .route("/networks", get(list_networks))
        .route("/networks/create", post(create_network))
        .route("/networks/{id}", delete(remove_network))
        .route("/networks/{id}/connect", post(connect_container))
        .route("/networks/{id}/disconnect", post(disconnect_container))
        .route("/networks/prune", post(prune_networks))
}

/// `GET /networks` — List networks.
async fn list_networks() -> Json<Vec<NetworkSummary>> {
    tracing::warn!("docker API: GET /networks — stub, returning empty list");
    Json(Vec::new())
}

/// `POST /networks/create` — Create a network.
async fn create_network() -> impl IntoResponse {
    tracing::warn!("docker API: POST /networks/create — stub");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": "network create not yet implemented"
        })),
    )
}

/// `DELETE /networks/{id}` — Remove a network.
async fn remove_network(Path(id): Path<String>) -> impl IntoResponse {
    tracing::warn!("docker API: DELETE /networks/{id} — stub");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": format!("network remove not yet implemented for {id}")
        })),
    )
}

/// `POST /networks/{id}/connect` — Connect a container to a network.
async fn connect_container(Path(id): Path<String>) -> impl IntoResponse {
    tracing::warn!("docker API: POST /networks/{id}/connect — stub");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": format!("network connect not yet implemented for {id}")
        })),
    )
}

/// `POST /networks/{id}/disconnect` — Disconnect a container from a network.
async fn disconnect_container(Path(id): Path<String>) -> impl IntoResponse {
    tracing::warn!("docker API: POST /networks/{id}/disconnect — stub");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": format!("network disconnect not yet implemented for {id}")
        })),
    )
}

/// `POST /networks/prune` — Prune unused networks.
async fn prune_networks() -> impl IntoResponse {
    tracing::warn!("docker API: POST /networks/prune — stub");
    Json(serde_json::json!({
        "NetworksDeleted": []
    }))
}
