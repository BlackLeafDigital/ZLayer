//! Container Device Interface (CDI) support.
//!
//! CDI is a vendor-neutral mechanism for declaring and injecting devices
//! into OCI containers. `ZLayer` discovers CDI specs from standard locations
//! (`/etc/cdi/`, `/var/run/cdi/`) and applies them to container specs
//! as an alternative to manual device passthrough.
//!
//! See: <https://github.com/cncf-tags/container-device-interface>

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// Standard CDI spec discovery directories
const CDI_SPEC_DIRS: &[&str] = &["/etc/cdi", "/var/run/cdi"];

/// A parsed CDI specification file
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CdiSpec {
    /// CDI spec version (e.g. "0.6.0")
    pub cdi_version: String,
    /// Device vendor and class (e.g. "nvidia.com/gpu")
    pub kind: String,
    /// Devices declared by this spec
    #[serde(default)]
    pub devices: Vec<CdiDevice>,
    /// Container edits applied to all devices of this kind
    #[serde(default)]
    pub container_edits: Option<CdiContainerEdits>,
}

/// A device within a CDI spec
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CdiDevice {
    /// Device name (e.g. "0" for GPU 0)
    pub name: String,
    /// Container edits specific to this device
    #[serde(default)]
    pub container_edits: Option<CdiContainerEdits>,
}

/// Modifications to apply to the OCI container spec
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CdiContainerEdits {
    /// Environment variables to add
    #[serde(default)]
    pub env: Vec<String>,
    /// Device nodes to create in the container
    #[serde(default)]
    pub device_nodes: Vec<CdiDeviceNode>,
    /// Mounts to add
    #[serde(default)]
    pub mounts: Vec<CdiMount>,
    /// Hooks to run
    #[serde(default)]
    pub hooks: Option<CdiHooks>,
}

/// A device node to create in the container
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CdiDeviceNode {
    /// Path inside the container
    pub path: String,
    /// Host path (defaults to container path)
    pub host_path: Option<String>,
    /// Device type: "b" (block) or "c" (char)
    #[serde(rename = "type")]
    pub device_type: Option<String>,
    /// Major device number
    pub major: Option<i64>,
    /// Minor device number
    pub minor: Option<i64>,
    /// File mode (e.g. 0o666)
    #[serde(default)]
    pub file_mode: Option<u32>,
    /// Owner UID
    pub uid: Option<u32>,
    /// Owner GID
    pub gid: Option<u32>,
    /// Device permissions ("rwm")
    pub permissions: Option<String>,
}

/// A mount to add to the container
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CdiMount {
    /// Container path
    pub container_path: String,
    /// Host path
    pub host_path: String,
    /// Mount options
    #[serde(default)]
    pub options: Vec<String>,
}

/// OCI lifecycle hooks
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CdiHooks {
    #[serde(default)]
    pub prestart: Vec<CdiHook>,
    #[serde(default)]
    pub create_runtime: Vec<CdiHook>,
    #[serde(default)]
    pub create_container: Vec<CdiHook>,
    #[serde(default)]
    pub start_container: Vec<CdiHook>,
    #[serde(default)]
    pub poststart: Vec<CdiHook>,
    #[serde(default)]
    pub poststop: Vec<CdiHook>,
}

/// A single OCI hook
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdiHook {
    pub path: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<String>,
}

/// Registry of all discovered CDI specs, indexed by fully-qualified device name.
///
/// A fully-qualified CDI device name has the format `vendor.com/class=device`,
/// e.g. `nvidia.com/gpu=0`.
#[derive(Debug, Default)]
pub struct CdiRegistry {
    /// All discovered specs, keyed by kind (e.g. "nvidia.com/gpu")
    specs: HashMap<String, CdiSpec>,
}

impl CdiRegistry {
    /// Discover and load CDI specs from the standard directories.
    ///
    /// Scans `/etc/cdi/` and `/var/run/cdi/` for `*.json` and `*.yaml` files,
    /// parses them, and indexes them by kind.
    pub fn discover() -> Self {
        let mut registry = Self::default();

        for dir in CDI_SPEC_DIRS {
            let dir_path = Path::new(dir);
            if !dir_path.is_dir() {
                debug!(dir = %dir, "CDI spec directory does not exist, skipping");
                continue;
            }

            let entries = match std::fs::read_dir(dir_path) {
                Ok(e) => e,
                Err(e) => {
                    warn!(dir = %dir, error = %e, "Failed to read CDI spec directory");
                    continue;
                }
            };

            for entry in entries.flatten() {
                let path = entry.path();
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                if ext != "json" && ext != "yaml" && ext != "yml" {
                    continue;
                }

                match Self::load_spec(&path) {
                    Ok(spec) => {
                        info!(
                            kind = %spec.kind,
                            devices = spec.devices.len(),
                            path = %path.display(),
                            "Loaded CDI spec"
                        );
                        registry.specs.insert(spec.kind.clone(), spec);
                    }
                    Err(e) => {
                        warn!(path = %path.display(), error = %e, "Failed to parse CDI spec");
                    }
                }
            }
        }

        registry
    }

    /// Load a single CDI spec file.
    fn load_spec(path: &Path) -> Result<CdiSpec, CdiError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| CdiError::Io(format!("{}: {e}", path.display())))?;

        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext == "json" {
            serde_json::from_str(&content)
                .map_err(|e| CdiError::Parse(format!("{}: {e}", path.display())))
        } else {
            serde_yaml::from_str(&content)
                .map_err(|e| CdiError::Parse(format!("{}: {e}", path.display())))
        }
    }

    /// Look up a CDI spec by kind (e.g. "nvidia.com/gpu").
    #[must_use]
    pub fn get_spec(&self, kind: &str) -> Option<&CdiSpec> {
        self.specs.get(kind)
    }

    /// Resolve a fully-qualified CDI device name to its container edits.
    ///
    /// Format: `vendor.com/class=device` (e.g. `nvidia.com/gpu=0`)
    ///
    /// Returns the merged container edits (global + device-specific) or None
    /// if the device is not found.
    #[must_use]
    pub fn resolve_device(&self, qualified_name: &str) -> Option<CdiContainerEdits> {
        let (kind, device_name) = qualified_name.split_once('=')?;
        let spec = self.specs.get(kind)?;
        let device = spec.devices.iter().find(|d| d.name == device_name)?;

        // Merge global and device-specific edits
        let mut merged = spec.container_edits.clone().unwrap_or_default();

        if let Some(ref dev_edits) = device.container_edits {
            merged.env.extend(dev_edits.env.clone());
            merged.device_nodes.extend(dev_edits.device_nodes.clone());
            merged.mounts.extend(dev_edits.mounts.clone());
        }

        Some(merged)
    }

    /// Check if any CDI specs are available.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }

    /// Get all available kinds.
    pub fn kinds(&self) -> impl Iterator<Item = &str> {
        self.specs.keys().map(String::as_str)
    }

    /// Generate a CDI spec for NVIDIA GPUs using nvidia-ctk.
    ///
    /// Runs `nvidia-ctk cdi generate` and returns the resulting spec,
    /// or None if nvidia-ctk is not available.
    pub async fn generate_nvidia_spec() -> Option<CdiSpec> {
        let output = tokio::process::Command::new("nvidia-ctk")
            .args(["cdi", "generate"])
            .output()
            .await
            .ok()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("nvidia-ctk cdi generate failed: {stderr}");
            return None;
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        match serde_yaml::from_str(&stdout) {
            Ok(spec) => {
                info!("Generated NVIDIA CDI spec via nvidia-ctk");
                Some(spec)
            }
            Err(e) => {
                warn!("Failed to parse nvidia-ctk output: {e}");
                None
            }
        }
    }
}

/// Errors from CDI operations
#[derive(Debug, thiserror::Error)]
pub enum CdiError {
    /// I/O error reading a CDI spec file
    #[error("CDI I/O error: {0}")]
    Io(String),
    /// Failed to parse a CDI spec file
    #[error("CDI parse error: {0}")]
    Parse(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_spec_json() -> &'static str {
        r#"{
            "cdiVersion": "0.6.0",
            "kind": "nvidia.com/gpu",
            "devices": [
                {
                    "name": "0",
                    "containerEdits": {
                        "env": ["NVIDIA_VISIBLE_DEVICES=0"],
                        "deviceNodes": [
                            {
                                "path": "/dev/nvidia0",
                                "hostPath": "/dev/nvidia0",
                                "type": "c",
                                "major": 195,
                                "minor": 0
                            }
                        ]
                    }
                },
                {
                    "name": "all",
                    "containerEdits": {
                        "env": ["NVIDIA_VISIBLE_DEVICES=all"]
                    }
                }
            ],
            "containerEdits": {
                "env": ["NVIDIA_DRIVER_CAPABILITIES=all"],
                "deviceNodes": [
                    {
                        "path": "/dev/nvidiactl",
                        "hostPath": "/dev/nvidiactl",
                        "type": "c"
                    }
                ],
                "mounts": [
                    {
                        "containerPath": "/usr/lib/x86_64-linux-gnu/libnvidia-ml.so.1",
                        "hostPath": "/usr/lib/x86_64-linux-gnu/libnvidia-ml.so.1",
                        "options": ["ro", "nosuid", "nodev", "bind"]
                    }
                ]
            }
        }"#
    }

    #[test]
    fn parse_cdi_spec_json() {
        let spec: CdiSpec = serde_json::from_str(sample_spec_json()).unwrap();
        assert_eq!(spec.cdi_version, "0.6.0");
        assert_eq!(spec.kind, "nvidia.com/gpu");
        assert_eq!(spec.devices.len(), 2);
        assert_eq!(spec.devices[0].name, "0");

        let global_edits = spec.container_edits.as_ref().unwrap();
        assert_eq!(global_edits.env, vec!["NVIDIA_DRIVER_CAPABILITIES=all"]);
        assert_eq!(global_edits.device_nodes.len(), 1);
        assert_eq!(global_edits.mounts.len(), 1);
    }

    #[test]
    fn resolve_device_merges_edits() {
        let spec: CdiSpec = serde_json::from_str(sample_spec_json()).unwrap();
        let mut registry = CdiRegistry::default();
        registry.specs.insert(spec.kind.clone(), spec);

        let edits = registry
            .resolve_device("nvidia.com/gpu=0")
            .expect("should resolve gpu 0");

        // Global env + device env
        assert!(edits
            .env
            .contains(&"NVIDIA_DRIVER_CAPABILITIES=all".to_string()));
        assert!(edits.env.contains(&"NVIDIA_VISIBLE_DEVICES=0".to_string()));

        // Global device node + device-specific device node
        assert_eq!(edits.device_nodes.len(), 2);

        // Global mount preserved
        assert_eq!(edits.mounts.len(), 1);
    }

    #[test]
    fn resolve_unknown_device_returns_none() {
        let registry = CdiRegistry::default();
        assert!(registry.resolve_device("nvidia.com/gpu=99").is_none());
    }

    #[test]
    fn resolve_malformed_name_returns_none() {
        let registry = CdiRegistry::default();
        assert!(registry.resolve_device("no-equals-sign").is_none());
    }

    #[test]
    fn empty_registry() {
        let registry = CdiRegistry::default();
        assert!(registry.is_empty());
        assert_eq!(registry.kinds().count(), 0);
    }

    #[test]
    fn parse_cdi_spec_yaml() {
        let yaml = r#"
cdiVersion: "0.6.0"
kind: "vendor.com/net"
devices:
  - name: "eth0"
    containerEdits:
      env:
        - "NET_DEVICE=eth0"
"#;
        let spec: CdiSpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(spec.kind, "vendor.com/net");
        assert_eq!(spec.devices.len(), 1);
        assert_eq!(spec.devices[0].name, "eth0");
    }
}
