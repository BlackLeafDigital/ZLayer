//! Server functions for Leptos SSR + Hydration
//!
//! These server functions provide data access from Leptos components.
//! The `#[server]` macro automatically:
//! - On the server (SSR): runs the function body with full access to backend services
//! - On the client (hydrate): generates an HTTP call to the server function endpoint
//!
//! This enables seamless data fetching that works identically whether
//! the component is rendered on the server or hydrated on the client.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

// ============================================================================
// Response Types
// ============================================================================

/// System statistics for the dashboard
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SystemStats {
    /// Version of ZLayer
    pub version: String,
    /// Number of running containers
    pub running_containers: u64,
    /// Number of available images
    pub available_images: u64,
    /// Number of configured overlay networks
    pub overlay_networks: u64,
}

/// Container specification summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerSpecSummary {
    pub id: String,
    pub name: String,
    pub image: String,
    pub status: String,
    pub created_at: String,
}

// ============================================================================
// Server Functions
// ============================================================================

/// Get system statistics
#[server(prefix = "/api/leptos")]
pub async fn get_system_stats() -> Result<SystemStats, ServerFnError> {
    // TODO: Connect to actual ZLayer services when available
    Ok(SystemStats {
        version: env!("CARGO_PKG_VERSION").to_string(),
        running_containers: 0,
        available_images: 0,
        overlay_networks: 0,
    })
}

/// Validate a container specification YAML
#[server(prefix = "/api/leptos")]
pub async fn validate_spec(yaml_content: String) -> Result<String, ServerFnError> {
    // TODO: Use zlayer-spec to validate when available
    if yaml_content.trim().is_empty() {
        return Err(ServerFnError::new("Specification cannot be empty"));
    }

    // Basic YAML parsing check
    match serde_yaml::from_str::<serde_json::Value>(&yaml_content) {
        Ok(_) => Ok("Specification is valid YAML".to_string()),
        Err(e) => Err(ServerFnError::new(format!("Invalid YAML: {}", e))),
    }
}
