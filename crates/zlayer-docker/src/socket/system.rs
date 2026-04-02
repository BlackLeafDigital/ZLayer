//! Docker Engine API system endpoints.
//!
//! Provides `/_ping`, `/version`, `/info`, `/events`, and `/system/df`.

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};

use super::types::{SystemInfo, VersionInfo};

/// System API routes.
pub fn routes() -> Router {
    Router::new()
        .route("/_ping", get(ping))
        .route("/version", get(version))
        .route("/info", get(info))
        .route("/events", get(events))
        .route("/system/df", get(disk_usage))
}

/// `GET /_ping` — Health check. Returns `"OK"` as plain text.
async fn ping() -> &'static str {
    "OK"
}

/// `GET /version` — Docker version info.
async fn version() -> Json<VersionInfo> {
    Json(VersionInfo {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        api_version: "1.43".to_owned(),
        min_api_version: "1.24".to_owned(),
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        kernel_version: kernel_version(),
        go_version: "N/A (ZLayer/Rust)".to_owned(),
    })
}

/// `GET /info` — System information.
async fn info() -> Json<SystemInfo> {
    tracing::warn!("docker API: GET /info — returning placeholder system info");
    Json(SystemInfo {
        containers: 0,
        containers_running: 0,
        containers_paused: 0,
        containers_stopped: 0,
        images: 0,
        name: hostname(),
        server_version: env!("CARGO_PKG_VERSION").to_owned(),
        operating_system: std::env::consts::OS.to_owned(),
        architecture: std::env::consts::ARCH.to_owned(),
        ncpu: num_cpus(),
        mem_total: total_memory(),
    })
}

/// `GET /events` — Event stream (stub).
async fn events() -> impl IntoResponse {
    tracing::warn!("docker API: GET /events — stub, not yet implemented");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": "event streaming not yet implemented"
        })),
    )
}

/// `GET /system/df` — Disk usage (stub).
async fn disk_usage() -> impl IntoResponse {
    tracing::warn!("docker API: GET /system/df — stub");
    Json(serde_json::json!({
        "LayersSize": 0,
        "Images": [],
        "Containers": [],
        "Volumes": [],
        "BuildCache": []
    }))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Best-effort hostname.
fn hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("HOST"))
        .unwrap_or_else(|_| "zlayer".to_owned())
}

/// Best-effort kernel version from `/proc/version` (Linux only).
fn kernel_version() -> String {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/version")
            .ok()
            .and_then(|v| v.split_whitespace().nth(2).map(String::from))
            .unwrap_or_else(|| "unknown".to_owned())
    }
    #[cfg(not(target_os = "linux"))]
    {
        "unknown".to_owned()
    }
}

/// Number of logical CPUs (best-effort).
fn num_cpus() -> i64 {
    std::thread::available_parallelism()
        .map(|n| i64::try_from(n.get()).unwrap_or(i64::MAX))
        .unwrap_or(1)
}

/// Total physical memory in bytes (Linux `/proc/meminfo`).
fn total_memory() -> i64 {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|info| {
                info.lines()
                    .find(|l| l.starts_with("MemTotal:"))
                    .and_then(|l| {
                        l.split_whitespace()
                            .nth(1)
                            .and_then(|kb| kb.parse::<i64>().ok())
                    })
                    // /proc/meminfo reports in kB
                    .map(|kb| kb * 1024)
            })
            .unwrap_or(0)
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}
