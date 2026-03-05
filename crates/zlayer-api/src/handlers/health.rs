//! Health check endpoints

use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Health check response
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct HealthResponse {
    /// Service status
    pub status: String,
    /// Service version
    pub version: String,
    /// Uptime in seconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime_secs: Option<u64>,
    /// Container runtime name (e.g. "youki", "mac-sandbox", "docker")
    #[serde(default = "default_runtime_name")]
    pub runtime_name: String,
}

/// Platform-based default runtime name.
///
/// On Linux the bundled libcontainer runtime ("youki") is preferred;
/// on macOS the sandbox runtime is tried first; Docker is the common fallback.
fn default_runtime_name() -> String {
    if cfg!(target_os = "linux") {
        "youki".to_string()
    } else if cfg!(target_os = "macos") {
        "mac-sandbox".to_string()
    } else {
        "auto".to_string()
    }
}

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
