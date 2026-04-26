//! Health check endpoints

use axum::Json;
pub use zlayer_types::api::health::*;

/// Liveness probe - basic health check
#[utoipa::path(
    get,
    path = "/health/live",
    responses(
        (status = 200, description = "Service is alive", body = HealthResponse),
    ),
    tag = "Health"
)]
pub async fn liveness() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_secs: None,
        runtime_name: default_runtime_name(),
    })
}

/// Readiness probe - full health check
#[utoipa::path(
    get,
    path = "/health/ready",
    responses(
        (status = 200, description = "Service is ready", body = HealthResponse),
        (status = 503, description = "Service not ready"),
    ),
    tag = "Health"
)]
pub async fn readiness() -> Json<HealthResponse> {
    // In production, check dependencies (database, scheduler, etc.)
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_secs: None,
        runtime_name: default_runtime_name(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_liveness() {
        let response = liveness().await;
        assert_eq!(response.status, "ok");
        assert!(!response.version.is_empty());
        assert!(!response.runtime_name.is_empty());
    }

    #[tokio::test]
    async fn test_readiness() {
        let response = readiness().await;
        assert_eq!(response.status, "ok");
        assert!(!response.runtime_name.is_empty());
    }
}
