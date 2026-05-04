//! Wire-format types for the Docker Engine API compatibility socket.
//!
//! These mirror Docker's exact JSON shapes (mostly `camelCase`, sometimes
//! `PascalCase` per Docker's conventions) and are translated into `ZLayer`'s
//! native DTOs by `super::translate`.

use std::collections::HashMap;

use serde::Serialize;

pub mod container_create;

/// Container summary for `GET /containers/json`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ContainerSummary {
    pub id: String,
    pub names: Vec<String>,
    pub image: String,
    pub image_id: String,
    pub command: String,
    pub created: i64,
    pub state: String,
    pub status: String,
    pub ports: Vec<PortBinding>,
    pub labels: HashMap<String, String>,
}

/// Port binding in a container summary.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PortBinding {
    #[serde(rename = "IP")]
    pub ip: String,
    pub private_port: u16,
    pub public_port: u16,
    #[serde(rename = "Type")]
    pub port_type: String,
}

// Note: `POST /containers/create`'s response shape is built inline in
// `super::containers::create_container` via `serde_json::json!`, since the
// daemon's `CreateContainerResponse` is what we ultimately translate back
// to Docker's `{Id, Warnings}` payload. No standalone struct needed.

/// Image summary for `GET /images/json`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ImageSummary {
    pub id: String,
    pub parent_id: String,
    pub repo_tags: Vec<String>,
    pub repo_digests: Vec<String>,
    pub created: i64,
    pub size: i64,
    pub virtual_size: i64,
    pub labels: HashMap<String, String>,
}

/// System info for `GET /info`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct SystemInfo {
    pub containers: i64,
    pub containers_running: i64,
    pub containers_paused: i64,
    pub containers_stopped: i64,
    pub images: i64,
    pub name: String,
    pub server_version: String,
    pub operating_system: String,
    pub architecture: String,
    #[serde(rename = "NCPU")]
    pub ncpu: i64,
    pub mem_total: i64,
}

/// Version info for `GET /version`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct VersionInfo {
    pub version: String,
    pub api_version: String,
    pub min_api_version: String,
    pub os: String,
    pub arch: String,
    pub kernel_version: String,
    pub go_version: String,
}

// Note: the strongly-typed `VolumeSummary` / `VolumeListResponse` /
// `NetworkSummary` / `IpamConfig` / `IpamPool` shells that previously
// lived here have been retired. The volumes and networks handlers now
// emit Docker's wire format directly via `serde_json::Value` so the
// PascalCase keys (`Status`, `Options`, `UsageData { Size, RefCount }`,
// `IPAM.Config[].Subnet`, ...) match Docker's exact 1.43 shape without
// requiring per-handler newtype wrappers. See
// `crates/zlayer-docker/src/socket/{volumes,networks}.rs`.
