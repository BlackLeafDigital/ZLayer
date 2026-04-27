//! Auth wire DTOs.
//!
//! These types describe how authentication for OCI registries is configured
//! over the wire. The synchronous and async resolvers themselves live in
//! `zlayer-core` (which depends on `oci-client` and other heavier crates);
//! this module intentionally only carries the serde-friendly DTOs.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Authentication source configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthSource {
    /// No authentication
    #[default]
    Anonymous,

    /// Basic authentication with username and password
    Basic { username: String, password: String },

    /// Load from Docker config.json
    DockerConfig,

    /// Load from environment variables
    EnvVar {
        username_var: String,
        password_var: String,
    },

    /// Look up credentials from the `RegistryCredentialStore` by id.
    /// Requires the async resolver -- the sync path returns `Anonymous` with
    /// a warning log.
    SecretStore { credential_id: String },
}

/// Per-registry authentication configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegistryAuthConfig {
    /// Registry hostname (e.g., "docker.io", "ghcr.io")
    pub registry: String,

    /// Authentication source for this registry
    pub source: AuthSource,
}

/// Global authentication configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuthConfig {
    /// Per-registry authentication overrides
    #[serde(default)]
    pub registries: Vec<RegistryAuthConfig>,

    /// Default authentication source for registries not in the list
    #[serde(default)]
    pub default: AuthSource,

    /// Custom path to Docker config.json (if not using default)
    pub docker_config_path: Option<PathBuf>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            registries: Vec::new(),
            default: AuthSource::DockerConfig,
            docker_config_path: None,
        }
    }
}
