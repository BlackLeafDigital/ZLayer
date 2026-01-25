//! Docker config.json authentication parser
//!
//! This module parses the Docker config file format used by Docker and other container tools
//! to store registry credentials. The config file is typically located at ~/.docker/config.json.

use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Docker config.json authentication manager
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerConfigAuth {
    #[serde(default)]
    auths: HashMap<String, AuthEntry>,
}

/// Authentication entry in Docker config
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AuthEntry {
    /// Base64-encoded "username:password"
    auth: Option<String>,
    /// Plain username (alternative to auth field)
    username: Option<String>,
    /// Plain password (alternative to auth field)
    password: Option<String>,
}

impl DockerConfigAuth {
    /// Load Docker config from the default location (~/.docker/config.json)
    pub fn load() -> Result<Self> {
        let path = Self::default_config_path()?;
        Self::load_from_path(&path)
    }

    /// Load Docker config from a specific path
    pub fn load_from_path(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self {
                auths: HashMap::new(),
            });
        }

        let contents = fs::read_to_string(path).map_err(|e| {
            crate::error::Error::config(format!("Failed to read Docker config: {}", e))
        })?;

        let config: DockerConfigAuth = serde_json::from_str(&contents).map_err(|e| {
            crate::error::Error::config(format!("Failed to parse Docker config: {}", e))
        })?;

        Ok(config)
    }

    /// Get credentials for a specific registry
    ///
    /// Returns (username, password) if credentials are found for the registry.
    /// The registry parameter should match the registry hostname (e.g., "docker.io", "ghcr.io").
    pub fn get_credentials(&self, registry: &str) -> Option<(String, String)> {
        // Try exact match first
        if let Some(entry) = self.auths.get(registry) {
            return Self::extract_credentials(entry);
        }

        // Try with https:// prefix
        let https_registry = format!("https://{}", registry);
        if let Some(entry) = self.auths.get(&https_registry) {
            return Self::extract_credentials(entry);
        }

        // Try index.docker.io for docker.io
        if registry == "docker.io" || registry == "registry-1.docker.io" {
            if let Some(entry) = self.auths.get("https://index.docker.io/v1/") {
                return Self::extract_credentials(entry);
            }
        }

        None
    }

    /// Extract credentials from an auth entry
    fn extract_credentials(entry: &AuthEntry) -> Option<(String, String)> {
        // If username and password are provided directly
        if let (Some(username), Some(password)) = (&entry.username, &entry.password) {
            return Some((username.clone(), password.clone()));
        }

        // If auth field is provided (base64 encoded "username:password")
        if let Some(auth) = &entry.auth {
            return Self::decode_auth(auth);
        }

        None
    }

    /// Decode base64-encoded "username:password" auth string
    fn decode_auth(auth: &str) -> Option<(String, String)> {
        use base64::Engine;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(auth)
            .ok()?;

        let decoded_str = String::from_utf8(decoded).ok()?;
        let parts: Vec<&str> = decoded_str.splitn(2, ':').collect();

        if parts.len() == 2 {
            Some((parts[0].to_string(), parts[1].to_string()))
        } else {
            None
        }
    }

    /// Get the default Docker config path (~/.docker/config.json)
    fn default_config_path() -> Result<PathBuf> {
        let home = dirs::home_dir().ok_or_else(|| {
            crate::error::Error::config("Cannot determine home directory".to_string())
        })?;

        Ok(home.join(".docker").join("config.json"))
    }

    /// Get all configured registry hostnames
    pub fn registries(&self) -> Vec<String> {
        self.auths.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_docker_config() {
        let config_json = r#"
        {
            "auths": {
                "ghcr.io": {
                    "auth": "dXNlcm5hbWU6cGFzc3dvcmQ="
                },
                "docker.io": {
                    "username": "myuser",
                    "password": "mypass"
                }
            }
        }
        "#;

        let config: DockerConfigAuth = serde_json::from_str(config_json).unwrap();

        // Test base64 auth field
        let (username, password) = config.get_credentials("ghcr.io").unwrap();
        assert_eq!(username, "username");
        assert_eq!(password, "password");

        // Test plain username/password
        let (username, password) = config.get_credentials("docker.io").unwrap();
        assert_eq!(username, "myuser");
        assert_eq!(password, "mypass");
    }

    #[test]
    fn test_registry_normalization() {
        let config_json = r#"
        {
            "auths": {
                "https://ghcr.io": {
                    "auth": "dXNlcm5hbWU6cGFzc3dvcmQ="
                },
                "https://index.docker.io/v1/": {
                    "auth": "ZG9ja2VyOnBhc3M="
                }
            }
        }
        "#;

        let config: DockerConfigAuth = serde_json::from_str(config_json).unwrap();

        // Should find with or without https://
        assert!(config.get_credentials("ghcr.io").is_some());

        // Should find docker.io credentials from index.docker.io
        assert!(config.get_credentials("docker.io").is_some());
    }

    #[test]
    fn test_decode_auth() {
        // "username:password" in base64
        let auth = "dXNlcm5hbWU6cGFzc3dvcmQ=";
        let (username, password) = DockerConfigAuth::decode_auth(auth).unwrap();
        assert_eq!(username, "username");
        assert_eq!(password, "password");
    }

    #[test]
    fn test_empty_config() {
        let config = DockerConfigAuth {
            auths: HashMap::new(),
        };

        assert!(config.get_credentials("docker.io").is_none());
    }
}
