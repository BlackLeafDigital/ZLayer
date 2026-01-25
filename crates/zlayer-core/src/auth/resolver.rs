//! Authentication resolver for OCI registries
//!
//! This module provides flexible authentication resolution supporting multiple sources
//! and per-registry configuration.

use super::DockerConfigAuth;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Authentication source configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthSource {
    /// No authentication
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
}

impl Default for AuthSource {
    fn default() -> Self {
        Self::Anonymous
    }
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

/// Authentication resolver that converts AuthConfig to oci_client RegistryAuth
pub struct AuthResolver {
    config: AuthConfig,
    docker_config: Option<DockerConfigAuth>,
    registry_map: HashMap<String, AuthSource>,
}

impl AuthResolver {
    /// Create a new authentication resolver
    pub fn new(config: AuthConfig) -> Self {
        // Build a map for fast registry lookups
        let registry_map: HashMap<String, AuthSource> = config
            .registries
            .iter()
            .map(|r| (r.registry.clone(), r.source.clone()))
            .collect();

        // Load Docker config if any source uses DockerConfig
        let needs_docker_config = config.default == AuthSource::DockerConfig
            || registry_map
                .values()
                .any(|s| matches!(s, AuthSource::DockerConfig));

        let docker_config = if needs_docker_config {
            Self::load_docker_config(&config.docker_config_path)
        } else {
            None
        };

        Self {
            config,
            docker_config,
            registry_map,
        }
    }

    /// Resolve authentication for an image reference
    ///
    /// Extracts the registry from the image reference and returns the appropriate
    /// oci_client::secrets::RegistryAuth.
    pub fn resolve(&self, image: &str) -> oci_client::secrets::RegistryAuth {
        let registry = Self::extract_registry(image);
        let source = self
            .registry_map
            .get(&registry)
            .unwrap_or(&self.config.default);

        self.resolve_source(source, &registry)
    }

    /// Resolve a specific AuthSource to RegistryAuth
    fn resolve_source(
        &self,
        source: &AuthSource,
        registry: &str,
    ) -> oci_client::secrets::RegistryAuth {
        match source {
            AuthSource::Anonymous => oci_client::secrets::RegistryAuth::Anonymous,

            AuthSource::Basic { username, password } => {
                oci_client::secrets::RegistryAuth::Basic(username.clone(), password.clone())
            }

            AuthSource::DockerConfig => {
                if let Some(ref docker_config) = self.docker_config {
                    if let Some((username, password)) = docker_config.get_credentials(registry) {
                        return oci_client::secrets::RegistryAuth::Basic(username, password);
                    }
                }
                // Fallback to anonymous if no credentials found
                oci_client::secrets::RegistryAuth::Anonymous
            }

            AuthSource::EnvVar {
                username_var,
                password_var,
            } => {
                let username = std::env::var(username_var).unwrap_or_default();
                let password = std::env::var(password_var).unwrap_or_default();

                if !username.is_empty() && !password.is_empty() {
                    oci_client::secrets::RegistryAuth::Basic(username, password)
                } else {
                    oci_client::secrets::RegistryAuth::Anonymous
                }
            }
        }
    }

    /// Extract registry hostname from image reference
    ///
    /// Examples:
    /// - "ubuntu:latest" -> "docker.io"
    /// - "ghcr.io/owner/repo:tag" -> "ghcr.io"
    /// - "localhost:5000/image" -> "localhost:5000"
    fn extract_registry(image: &str) -> String {
        // Remove digest if present
        let image_without_digest = image.split('@').next().unwrap_or(image);

        // Split by '/'
        let parts: Vec<&str> = image_without_digest.split('/').collect();

        // If there's no '/', it's just an image name, assume Docker Hub
        if parts.len() == 1 {
            return "docker.io".to_string();
        }

        // Check if first part looks like a hostname (contains '.' or ':' or is 'localhost')
        let first_part = parts[0];
        if first_part.contains('.') || first_part.contains(':') || first_part == "localhost" {
            first_part.to_string()
        } else {
            // No explicit registry (e.g., "library/ubuntu"), assume Docker Hub
            "docker.io".to_string()
        }
    }

    /// Load Docker config from path or default location
    fn load_docker_config(path: &Option<PathBuf>) -> Option<DockerConfigAuth> {
        let config = if let Some(path) = path {
            DockerConfigAuth::load_from_path(path).ok()
        } else {
            DockerConfigAuth::load().ok()
        };

        if config.is_none() {
            tracing::debug!("Failed to load Docker config, using anonymous auth as fallback");
        }

        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_registry() {
        assert_eq!(AuthResolver::extract_registry("ubuntu"), "docker.io");
        assert_eq!(AuthResolver::extract_registry("ubuntu:latest"), "docker.io");
        assert_eq!(
            AuthResolver::extract_registry("library/ubuntu"),
            "docker.io"
        );
        assert_eq!(
            AuthResolver::extract_registry("ghcr.io/owner/repo"),
            "ghcr.io"
        );
        assert_eq!(
            AuthResolver::extract_registry("ghcr.io/owner/repo:tag"),
            "ghcr.io"
        );
        assert_eq!(
            AuthResolver::extract_registry("localhost:5000/image"),
            "localhost:5000"
        );
        assert_eq!(
            AuthResolver::extract_registry("myregistry.com/path/to/image:v1.0"),
            "myregistry.com"
        );
    }

    #[test]
    fn test_anonymous_auth() {
        let config = AuthConfig {
            default: AuthSource::Anonymous,
            ..Default::default()
        };

        let resolver = AuthResolver::new(config);
        let auth = resolver.resolve("ubuntu:latest");

        assert!(matches!(auth, oci_client::secrets::RegistryAuth::Anonymous));
    }

    #[test]
    fn test_basic_auth() {
        let config = AuthConfig {
            default: AuthSource::Basic {
                username: "user".to_string(),
                password: "pass".to_string(),
            },
            ..Default::default()
        };

        let resolver = AuthResolver::new(config);
        let auth = resolver.resolve("ubuntu:latest");

        match auth {
            oci_client::secrets::RegistryAuth::Basic(username, password) => {
                assert_eq!(username, "user");
                assert_eq!(password, "pass");
            }
            _ => panic!("Expected Basic auth"),
        }
    }

    #[test]
    fn test_per_registry_auth() {
        let config = AuthConfig {
            registries: vec![RegistryAuthConfig {
                registry: "ghcr.io".to_string(),
                source: AuthSource::Basic {
                    username: "ghcr_user".to_string(),
                    password: "ghcr_pass".to_string(),
                },
            }],
            default: AuthSource::Anonymous,
            ..Default::default()
        };

        let resolver = AuthResolver::new(config);

        // Should use specific auth for ghcr.io
        let auth = resolver.resolve("ghcr.io/owner/repo:tag");
        match auth {
            oci_client::secrets::RegistryAuth::Basic(username, password) => {
                assert_eq!(username, "ghcr_user");
                assert_eq!(password, "ghcr_pass");
            }
            _ => panic!("Expected Basic auth for ghcr.io"),
        }

        // Should use default (anonymous) for docker.io
        let auth = resolver.resolve("ubuntu:latest");
        assert!(matches!(auth, oci_client::secrets::RegistryAuth::Anonymous));
    }

    #[test]
    fn test_env_var_auth() {
        std::env::set_var("TEST_USERNAME", "env_user");
        std::env::set_var("TEST_PASSWORD", "env_pass");

        let config = AuthConfig {
            default: AuthSource::EnvVar {
                username_var: "TEST_USERNAME".to_string(),
                password_var: "TEST_PASSWORD".to_string(),
            },
            ..Default::default()
        };

        let resolver = AuthResolver::new(config);
        let auth = resolver.resolve("ubuntu:latest");

        match auth {
            oci_client::secrets::RegistryAuth::Basic(username, password) => {
                assert_eq!(username, "env_user");
                assert_eq!(password, "env_pass");
            }
            _ => panic!("Expected Basic auth from env vars"),
        }

        std::env::remove_var("TEST_USERNAME");
        std::env::remove_var("TEST_PASSWORD");
    }

    #[test]
    fn test_env_var_auth_fallback() {
        // Test that missing env vars fall back to anonymous
        let config = AuthConfig {
            default: AuthSource::EnvVar {
                username_var: "NONEXISTENT_USER".to_string(),
                password_var: "NONEXISTENT_PASS".to_string(),
            },
            ..Default::default()
        };

        let resolver = AuthResolver::new(config);
        let auth = resolver.resolve("ubuntu:latest");

        assert!(matches!(auth, oci_client::secrets::RegistryAuth::Anonymous));
    }
}
