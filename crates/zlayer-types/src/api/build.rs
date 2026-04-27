//! Build API DTOs.
//!
//! Wire types for the build endpoints. These are the request/response
//! shapes consumed by both the daemon and SDK clients.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Build request for JSON API
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct BuildRequest {
    /// Use runtime template instead of Dockerfile
    #[serde(default)]
    pub runtime: Option<String>,
    /// Build arguments (ARG values)
    #[serde(default)]
    pub build_args: HashMap<String, String>,
    /// Target stage for multi-stage builds
    #[serde(default)]
    pub target: Option<String>,
    /// Tags to apply to the image
    #[serde(default)]
    pub tags: Vec<String>,
    /// Disable cache
    #[serde(default)]
    pub no_cache: bool,
    /// Push to registry after build
    #[serde(default)]
    pub push: bool,
}

/// Build status response
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct BuildStatus {
    /// Unique build ID
    pub id: String,
    /// Current build status
    pub status: BuildStateEnum,
    /// Image ID (if completed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_id: Option<String>,
    /// Error message (if failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// When the build started (ISO 8601)
    pub started_at: String,
    /// When the build completed (ISO 8601)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

/// Build state enumeration
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BuildStateEnum {
    /// Build is queued
    Pending,
    /// Build is running
    Running,
    /// Build completed successfully
    Complete,
    /// Build failed
    Failed,
}

/// Runtime template information
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TemplateInfo {
    /// Template name (e.g., "node20")
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// Files that indicate this runtime should be used
    pub detect_files: Vec<String>,
}

/// Trigger build response
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TriggerBuildResponse {
    /// Unique build ID for tracking
    pub build_id: String,
    /// Human-readable message
    pub message: String,
}

/// Build event wrapper for SSE serialization
#[derive(Debug, Clone, Serialize)]
pub struct BuildEventWrapper {
    /// Event type
    #[serde(rename = "type")]
    pub event_type: String,
    /// Event data
    pub data: serde_json::Value,
}

/// Build request with server-side context path
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct BuildRequestWithContext {
    /// Path to the build context on the server
    pub context_path: String,
    /// Use runtime template instead of Dockerfile
    #[serde(default)]
    pub runtime: Option<String>,
    /// Build arguments
    #[serde(default)]
    pub build_args: HashMap<String, String>,
    /// Target stage
    #[serde(default)]
    pub target: Option<String>,
    /// Tags to apply
    #[serde(default)]
    pub tags: Vec<String>,
    /// Disable cache
    #[serde(default)]
    pub no_cache: bool,
    /// Push after build
    #[serde(default)]
    pub push: bool,
}

impl From<BuildRequestWithContext> for BuildRequest {
    fn from(req: BuildRequestWithContext) -> Self {
        Self {
            runtime: req.runtime,
            build_args: req.build_args,
            target: req.target,
            tags: req.tags,
            no_cache: req.no_cache,
            push: req.push,
        }
    }
}
