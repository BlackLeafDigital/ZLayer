//! Docker Engine API v1.43 JSON response types.
//!
//! These types match Docker's actual JSON responses so that Docker-aware
//! tools (VS Code Docker extension, CI systems, etc.) can parse them.

use std::collections::HashMap;

use serde::Serialize;

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

/// Container create response for `POST /containers/create`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ContainerCreateResponse {
    pub id: String,
    pub warnings: Vec<String>,
}

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

/// Volume summary.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct VolumeSummary {
    pub name: String,
    pub driver: String,
    pub mountpoint: String,
    pub labels: HashMap<String, String>,
    pub scope: String,
}

/// Volume list response for `GET /volumes`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct VolumeListResponse {
    pub volumes: Vec<VolumeSummary>,
    pub warnings: Vec<String>,
}

/// Network summary for `GET /networks`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct NetworkSummary {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub scope: String,
    #[serde(rename = "IPAM")]
    pub ipam: IpamConfig,
    pub labels: HashMap<String, String>,
}

/// IPAM configuration for a network.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct IpamConfig {
    pub driver: String,
    pub config: Vec<IpamPool>,
}

/// IPAM address pool.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct IpamPool {
    pub subnet: String,
    pub gateway: String,
}
