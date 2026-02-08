//! OCI image configuration parsing
//!
//! Parses the OCI image config blob to extract container runtime defaults
//! such as `Entrypoint`, `Cmd`, `WorkingDir`, `Env`, and `User`.
//!
//! The OCI image config JSON has the following structure:
//! ```json
//! {
//!   "config": {
//!     "Entrypoint": ["/usr/bin/myapp"],
//!     "Cmd": ["--port", "8080"],
//!     "WorkingDir": "/app",
//!     "Env": ["PATH=/usr/local/bin:/usr/bin"],
//!     "User": "1000:1000",
//!     "ExposedPorts": {"8080/tcp": {}},
//!     "Volumes": {"/data": {}},
//!     "Labels": {"maintainer": "someone"}
//!   },
//!   "rootfs": { ... },
//!   "history": [ ... ]
//! }
//! ```

use serde::Deserialize;
use std::collections::HashMap;

/// Container runtime configuration extracted from an OCI image config blob.
///
/// This represents the `config` object inside the OCI image configuration JSON.
/// Fields map to the corresponding Docker/OCI image config spec fields.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ImageConfig {
    /// The entrypoint for the container (e.g., `["/usr/bin/myapp"]`).
    ///
    /// When both `entrypoint` and `cmd` are set, `cmd` provides default
    /// arguments to the entrypoint.
    #[serde(rename = "Entrypoint")]
    pub entrypoint: Option<Vec<String>>,

    /// The default command for the container (e.g., `["--port", "8080"]`).
    ///
    /// If `entrypoint` is also set, `cmd` provides default arguments.
    /// If `entrypoint` is not set, `cmd` is the full command to run.
    #[serde(rename = "Cmd")]
    pub cmd: Option<Vec<String>>,

    /// The working directory for the container process.
    #[serde(rename = "WorkingDir")]
    pub working_dir: Option<String>,

    /// Environment variables in `KEY=VALUE` format.
    #[serde(rename = "Env")]
    pub env: Option<Vec<String>>,

    /// The user (and optionally group) to run the container process as.
    /// Format: `"user"`, `"user:group"`, `"uid"`, or `"uid:gid"`.
    #[serde(rename = "User")]
    pub user: Option<String>,

    /// Ports exposed by the container. Keys are in `"port/protocol"` format
    /// (e.g., `"8080/tcp"`). Values are empty objects.
    #[serde(rename = "ExposedPorts")]
    pub exposed_ports: Option<HashMap<String, serde_json::Value>>,

    /// Volumes defined by the image. Keys are mount paths, values are empty objects.
    #[serde(rename = "Volumes")]
    pub volumes: Option<HashMap<String, serde_json::Value>>,

    /// Arbitrary metadata labels.
    #[serde(rename = "Labels")]
    pub labels: Option<HashMap<String, String>>,

    /// Signal to send to the container to stop it (e.g., `"SIGTERM"`).
    #[serde(rename = "StopSignal")]
    pub stop_signal: Option<String>,
}

impl ImageConfig {
    /// Build the full command line by combining entrypoint and cmd.
    ///
    /// Follows Docker/OCI semantics:
    /// - If entrypoint is set, it is the executable and cmd provides default args
    /// - If only cmd is set, it is the full command
    /// - If neither is set, returns None
    pub fn full_command(&self) -> Option<Vec<String>> {
        match (&self.entrypoint, &self.cmd) {
            (Some(ep), Some(cmd)) => {
                let mut full = ep.clone();
                full.extend(cmd.iter().cloned());
                Some(full)
            }
            (Some(ep), None) => Some(ep.clone()),
            (None, Some(cmd)) => Some(cmd.clone()),
            (None, None) => None,
        }
    }
}

/// Top-level OCI image configuration JSON structure.
///
/// This wraps the actual container config along with rootfs and history
/// information. We only need the `config` field for runtime defaults.
#[derive(Debug, Deserialize)]
pub(crate) struct OciImageConfigRoot {
    /// The container runtime configuration.
    pub config: Option<ImageConfig>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_full_config() {
        let json = r#"{
            "config": {
                "Entrypoint": ["/usr/bin/node"],
                "Cmd": ["server.js"],
                "WorkingDir": "/app",
                "Env": ["NODE_ENV=production", "PORT=3000"],
                "User": "1000:1000",
                "ExposedPorts": {"3000/tcp": {}},
                "Volumes": {"/data": {}},
                "Labels": {"maintainer": "test@example.com"},
                "StopSignal": "SIGTERM"
            },
            "rootfs": {
                "type": "layers",
                "diff_ids": ["sha256:abc123"]
            }
        }"#;

        let root: OciImageConfigRoot = serde_json::from_str(json).unwrap();
        let config = root.config.unwrap();

        assert_eq!(config.entrypoint, Some(vec!["/usr/bin/node".to_string()]));
        assert_eq!(config.cmd, Some(vec!["server.js".to_string()]));
        assert_eq!(config.working_dir, Some("/app".to_string()));
        assert_eq!(
            config.env,
            Some(vec![
                "NODE_ENV=production".to_string(),
                "PORT=3000".to_string()
            ])
        );
        assert_eq!(config.user, Some("1000:1000".to_string()));
        assert!(config.exposed_ports.is_some());
        assert!(config.exposed_ports.unwrap().contains_key("3000/tcp"));
        assert!(config.volumes.is_some());
        assert!(config.volumes.unwrap().contains_key("/data"));
        assert_eq!(
            config.labels.as_ref().unwrap().get("maintainer"),
            Some(&"test@example.com".to_string())
        );
        assert_eq!(config.stop_signal, Some("SIGTERM".to_string()));
    }

    #[test]
    fn test_parse_minimal_config() {
        let json = r#"{
            "config": {},
            "rootfs": {
                "type": "layers",
                "diff_ids": []
            }
        }"#;

        let root: OciImageConfigRoot = serde_json::from_str(json).unwrap();
        let config = root.config.unwrap();

        assert!(config.entrypoint.is_none());
        assert!(config.cmd.is_none());
        assert!(config.working_dir.is_none());
        assert!(config.env.is_none());
        assert!(config.user.is_none());
    }

    #[test]
    fn test_parse_null_config() {
        let json = r#"{
            "config": null,
            "rootfs": {
                "type": "layers",
                "diff_ids": []
            }
        }"#;

        let root: OciImageConfigRoot = serde_json::from_str(json).unwrap();
        assert!(root.config.is_none());
    }

    #[test]
    fn test_parse_missing_config() {
        // Some images might not have a config section at all
        let json = r#"{
            "rootfs": {
                "type": "layers",
                "diff_ids": []
            }
        }"#;

        let root: OciImageConfigRoot = serde_json::from_str(json).unwrap();
        assert!(root.config.is_none());
    }

    #[test]
    fn test_parse_docker_hub_nginx_like() {
        // Realistic config from a Docker Hub nginx image
        let json = r#"{
            "config": {
                "Entrypoint": ["/docker-entrypoint.sh"],
                "Cmd": ["nginx", "-g", "daemon off;"],
                "WorkingDir": "",
                "Env": [
                    "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
                    "NGINX_VERSION=1.25.3"
                ],
                "ExposedPorts": {
                    "80/tcp": {}
                },
                "StopSignal": "SIGQUIT"
            },
            "rootfs": {
                "type": "layers",
                "diff_ids": [
                    "sha256:aaa",
                    "sha256:bbb"
                ]
            }
        }"#;

        let root: OciImageConfigRoot = serde_json::from_str(json).unwrap();
        let config = root.config.unwrap();

        assert_eq!(
            config.entrypoint,
            Some(vec!["/docker-entrypoint.sh".to_string()])
        );
        assert_eq!(
            config.cmd,
            Some(vec![
                "nginx".to_string(),
                "-g".to_string(),
                "daemon off;".to_string()
            ])
        );
        // Empty string working_dir should deserialize as Some("")
        assert_eq!(config.working_dir, Some(String::new()));
        assert_eq!(config.stop_signal, Some("SIGQUIT".to_string()));
    }

    #[test]
    fn test_parse_cmd_only_image() {
        // Images like ubuntu:latest only have Cmd, no Entrypoint
        let json = r#"{
            "config": {
                "Cmd": ["/bin/bash"],
                "Env": ["PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"]
            },
            "rootfs": {
                "type": "layers",
                "diff_ids": ["sha256:abc"]
            }
        }"#;

        let root: OciImageConfigRoot = serde_json::from_str(json).unwrap();
        let config = root.config.unwrap();

        assert!(config.entrypoint.is_none());
        assert_eq!(config.cmd, Some(vec!["/bin/bash".to_string()]));
    }

    #[test]
    fn test_full_command_both_set() {
        let config = ImageConfig {
            entrypoint: Some(vec!["/usr/bin/node".to_string()]),
            cmd: Some(vec![
                "server.js".to_string(),
                "--port".to_string(),
                "3000".to_string(),
            ]),
            ..Default::default()
        };

        assert_eq!(
            config.full_command(),
            Some(vec![
                "/usr/bin/node".to_string(),
                "server.js".to_string(),
                "--port".to_string(),
                "3000".to_string(),
            ])
        );
    }

    #[test]
    fn test_full_command_entrypoint_only() {
        let config = ImageConfig {
            entrypoint: Some(vec!["/usr/bin/myapp".to_string()]),
            ..Default::default()
        };

        assert_eq!(
            config.full_command(),
            Some(vec!["/usr/bin/myapp".to_string()])
        );
    }

    #[test]
    fn test_full_command_cmd_only() {
        let config = ImageConfig {
            cmd: Some(vec!["/bin/bash".to_string()]),
            ..Default::default()
        };

        assert_eq!(config.full_command(), Some(vec!["/bin/bash".to_string()]));
    }

    #[test]
    fn test_full_command_neither_set() {
        let config = ImageConfig::default();
        assert!(config.full_command().is_none());
    }

    #[test]
    fn test_image_config_default() {
        let config = ImageConfig::default();
        assert!(config.entrypoint.is_none());
        assert!(config.cmd.is_none());
        assert!(config.working_dir.is_none());
        assert!(config.env.is_none());
        assert!(config.user.is_none());
        assert!(config.exposed_ports.is_none());
        assert!(config.volumes.is_none());
        assert!(config.labels.is_none());
        assert!(config.stop_signal.is_none());
    }

    #[test]
    fn test_image_config_clone() {
        let config = ImageConfig {
            entrypoint: Some(vec!["/bin/sh".to_string()]),
            cmd: Some(vec!["-c".to_string(), "echo hello".to_string()]),
            working_dir: Some("/tmp".to_string()),
            env: Some(vec!["FOO=bar".to_string()]),
            user: Some("nobody".to_string()),
            exposed_ports: None,
            volumes: None,
            labels: Some(HashMap::from([("key".to_string(), "value".to_string())])),
            stop_signal: None,
        };

        let cloned = config.clone();
        assert_eq!(cloned.entrypoint, config.entrypoint);
        assert_eq!(cloned.cmd, config.cmd);
        assert_eq!(cloned.working_dir, config.working_dir);
        assert_eq!(cloned.env, config.env);
        assert_eq!(cloned.user, config.user);
        assert_eq!(cloned.labels, config.labels);
    }

    #[test]
    fn test_parse_extra_fields_ignored() {
        // The OCI config has many more fields (architecture, os, etc.)
        // that we don't care about. Verify they are silently ignored.
        let json = r#"{
            "architecture": "amd64",
            "os": "linux",
            "config": {
                "Entrypoint": ["/app"],
                "Hostname": "",
                "Domainname": "",
                "AttachStdout": false,
                "Tty": false,
                "ArgsEscaped": true
            },
            "rootfs": {
                "type": "layers",
                "diff_ids": ["sha256:abc"]
            },
            "history": [
                {"created": "2024-01-01T00:00:00Z", "created_by": "COPY . /app"}
            ]
        }"#;

        let root: OciImageConfigRoot = serde_json::from_str(json).unwrap();
        let config = root.config.unwrap();
        assert_eq!(config.entrypoint, Some(vec!["/app".to_string()]));
    }
}
