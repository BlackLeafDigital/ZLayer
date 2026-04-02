//! Docker Engine API container endpoints.
//!
//! Stub implementations that return placeholder data. Each handler logs
//! a warning when called so that usage of unimplemented endpoints is
//! visible in traces.

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};

use super::types::{ContainerCreateResponse, ContainerSummary};

/// Container API routes.
pub fn routes() -> Router {
    Router::new()
        .route("/containers/json", get(list_containers))
        .route("/containers/create", post(create_container))
        .route("/containers/{id}/start", post(start_container))
        .route("/containers/{id}/stop", post(stop_container))
        .route("/containers/{id}/kill", post(kill_container))
        .route("/containers/{id}", delete(remove_container))
        .route("/containers/{id}/json", get(inspect_container))
        .route("/containers/{id}/logs", get(container_logs))
        .route("/containers/{id}/stats", get(container_stats))
        .route("/containers/{id}/wait", post(wait_container))
        .route("/containers/{id}/exec", post(create_exec))
        .route("/exec/{id}/start", post(start_exec))
}

/// `GET /containers/json` — List containers.
async fn list_containers() -> Json<Vec<ContainerSummary>> {
    tracing::warn!("docker API: GET /containers/json — stub, returning empty list");
    Json(Vec::new())
}

/// `POST /containers/create` — Create a container.
async fn create_container() -> Json<ContainerCreateResponse> {
    tracing::warn!("docker API: POST /containers/create — stub");
    Json(ContainerCreateResponse {
        id: "zlayer-stub-container-id".to_owned(),
        warnings: vec!["ZLayer Docker API emulation: container create is a stub".to_owned()],
    })
}

/// `POST /containers/{id}/start` — Start a container.
async fn start_container(Path(id): Path<String>) -> impl IntoResponse {
    tracing::warn!("docker API: POST /containers/{id}/start — stub");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": format!("container start not yet implemented for {id}")
        })),
    )
}

/// `POST /containers/{id}/stop` — Stop a container.
async fn stop_container(Path(id): Path<String>) -> impl IntoResponse {
    tracing::warn!("docker API: POST /containers/{id}/stop — stub");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": format!("container stop not yet implemented for {id}")
        })),
    )
}

/// `POST /containers/{id}/kill` — Kill a container.
async fn kill_container(Path(id): Path<String>) -> impl IntoResponse {
    tracing::warn!("docker API: POST /containers/{id}/kill — stub");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": format!("container kill not yet implemented for {id}")
        })),
    )
}

/// `DELETE /containers/{id}` — Remove a container.
async fn remove_container(Path(id): Path<String>) -> impl IntoResponse {
    tracing::warn!("docker API: DELETE /containers/{id} — stub");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": format!("container remove not yet implemented for {id}")
        })),
    )
}

/// `GET /containers/{id}/json` — Inspect a container.
async fn inspect_container(Path(id): Path<String>) -> impl IntoResponse {
    tracing::warn!("docker API: GET /containers/{id}/json — stub");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": format!("container inspect not yet implemented for {id}")
        })),
    )
}

/// `GET /containers/{id}/logs` — Get container logs.
async fn container_logs(Path(id): Path<String>) -> impl IntoResponse {
    tracing::warn!("docker API: GET /containers/{id}/logs — stub");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": format!("container logs not yet implemented for {id}")
        })),
    )
}

/// `GET /containers/{id}/stats` — Get container stats.
async fn container_stats(Path(id): Path<String>) -> impl IntoResponse {
    tracing::warn!("docker API: GET /containers/{id}/stats — stub");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": format!("container stats not yet implemented for {id}")
        })),
    )
}

/// `POST /containers/{id}/wait` — Wait for a container.
async fn wait_container(Path(id): Path<String>) -> impl IntoResponse {
    tracing::warn!("docker API: POST /containers/{id}/wait — stub");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": format!("container wait not yet implemented for {id}")
        })),
    )
}

/// `POST /containers/{id}/exec` — Create an exec instance.
async fn create_exec(Path(id): Path<String>) -> impl IntoResponse {
    tracing::warn!("docker API: POST /containers/{id}/exec — stub");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": format!("exec create not yet implemented for container {id}")
        })),
    )
}

/// `POST /exec/{id}/start` — Start an exec instance.
async fn start_exec(Path(id): Path<String>) -> impl IntoResponse {
    tracing::warn!("docker API: POST /exec/{id}/start — stub");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": format!("exec start not yet implemented for {id}")
        })),
    )
}
