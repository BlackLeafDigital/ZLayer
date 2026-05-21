//! Container Device Interface (CDI) support.
//!
//! CDI is a vendor-neutral mechanism for declaring and injecting devices
//! into OCI containers. `ZLayer` discovers CDI specs from standard locations
//! (`/etc/cdi/`, `/var/run/cdi/`) and applies them to container specs
//! as an alternative to manual device passthrough.
//!
//! See: <https://github.com/cncf-tags/container-device-interface>

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// Standard CDI spec discovery directories
const CDI_SPEC_DIRS: &[&str] = &["/etc/cdi", "/var/run/cdi"];

/// Environment variable that overrides the default CDI spec search path.
///
/// When set, its value is interpreted as a list of directories separated by
/// the platform path separator (`:` on Unix, `;` on Windows). Each directory
/// is scanned in addition to the standard locations.
pub const CDI_SPEC_DIRS_ENV: &str = "CDI_SPEC_DIRS";

/// Map a `GpuSpec.vendor` short name to a CDI kind.
///
/// CDI kinds are fully-qualified (`vendor.tld/class`) while `GpuSpec.vendor`
/// is a short alias (`"nvidia"`, `"amd"`, `"intel"`). This is the canonical
/// mapping used when resolving GPU devices from a service spec.
#[must_use]
pub fn vendor_to_cdi_kind(vendor: &str) -> Option<&'static str> {
    match vendor {
        "nvidia" => Some("nvidia.com/gpu"),
        "amd" => Some("amd.com/gpu"),
        "intel" => Some("intel.com/gpu"),
        _ => None,
    }
}

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
    /// parses them, and indexes them by kind. Honors the `CDI_SPEC_DIRS`
    /// environment variable for an additional override search path.
    pub fn discover() -> Self {
        let mut dirs: Vec<PathBuf> = CDI_SPEC_DIRS.iter().map(PathBuf::from).collect();
        if let Ok(env_dirs) = std::env::var(CDI_SPEC_DIRS_ENV) {
            for entry in std::env::split_paths(&env_dirs) {
                if !entry.as_os_str().is_empty() {
                    dirs.push(entry);
                }
            }
        }
        Self::discover_from(&dirs)
    }

    /// Discover and load CDI specs from an explicit list of directories.
    ///
    /// This is primarily useful for tests where the standard system paths
    /// are not appropriate. Missing directories are silently skipped.
    pub fn discover_from<P: AsRef<Path>>(dirs: &[P]) -> Self {
        let mut registry = Self::default();

        for dir in dirs {
            let dir_path = dir.as_ref();
            if !dir_path.is_dir() {
                debug!(dir = %dir_path.display(), "CDI spec directory does not exist, skipping");
                continue;
            }

            let entries = match std::fs::read_dir(dir_path) {
                Ok(e) => e,
                Err(e) => {
                    warn!(dir = %dir_path.display(), error = %e, "Failed to read CDI spec directory");
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
            if let Some(ref dev_hooks) = dev_edits.hooks {
                let merged_hooks = merged.hooks.get_or_insert_with(CdiHooks::default);
                merged_hooks.prestart.extend(dev_hooks.prestart.clone());
                merged_hooks
                    .create_runtime
                    .extend(dev_hooks.create_runtime.clone());
                merged_hooks
                    .create_container
                    .extend(dev_hooks.create_container.clone());
                merged_hooks
                    .start_container
                    .extend(dev_hooks.start_container.clone());
                merged_hooks.poststart.extend(dev_hooks.poststart.clone());
                merged_hooks.poststop.extend(dev_hooks.poststop.clone());
            }
        }

        Some(merged)
    }

    /// Resolve one or more device names for a given vendor kind into a
    /// flat list of per-device container edits.
    ///
    /// The special device name `"all"` expands to every device declared in
    /// the spec for the requested vendor — this mirrors the semantics of
    /// `NVIDIA_VISIBLE_DEVICES=all` and matches the behavior of `nvidia-ctk`'s
    /// CDI implementation.
    ///
    /// # Errors
    ///
    /// Returns [`CdiError::SpecMissing`] if no spec is loaded for the
    /// requested kind. Returns [`CdiError::DeviceMissing`] if any of the
    /// requested device names is not declared in the spec. Returns
    /// [`CdiError::NoDevices`] if the request asks for `"all"` but the
    /// spec contains no devices.
    pub fn resolve_for_kind(
        &self,
        kind: &str,
        device_names: &[String],
    ) -> std::result::Result<Vec<CdiContainerEdits>, CdiError> {
        let spec = self
            .specs
            .get(kind)
            .ok_or_else(|| CdiError::SpecMissing(kind.to_string()))?;

        // Expand the "all" alias to every declared device name (excluding
        // any device that itself is literally named "all" — that's a
        // sentinel device used by some vendors to express "use any GPU").
        let expanded: Vec<String> = if device_names.iter().any(|n| n == "all") {
            let names: Vec<String> = spec
                .devices
                .iter()
                .filter(|d| d.name != "all")
                .map(|d| d.name.clone())
                .collect();
            if names.is_empty() {
                return Err(CdiError::NoDevices(kind.to_string()));
            }
            names
        } else {
            device_names.to_vec()
        };

        let mut out = Vec::with_capacity(expanded.len());
        for name in &expanded {
            let qualified = format!("{kind}={name}");
            let edits = self
                .resolve_device(&qualified)
                .ok_or_else(|| CdiError::DeviceMissing {
                    kind: kind.to_string(),
                    device: name.clone(),
                })?;
            out.push(edits);
        }
        Ok(out)
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
    /// No CDI spec installed for the requested vendor/kind.
    ///
    /// Typically means the vendor's CDI generator has not been run on this
    /// host (e.g. `nvidia-ctk cdi generate --output=/etc/cdi/nvidia.json`).
    #[error("no CDI spec installed for kind '{0}' (run the vendor's CDI generator)")]
    SpecMissing(String),
    /// A requested device name is not declared in the CDI spec for the kind.
    #[error("CDI device '{device}' not declared in spec for kind '{kind}'")]
    DeviceMissing {
        /// CDI kind (e.g. `nvidia.com/gpu`).
        kind: String,
        /// Device name that was requested but not found.
        device: String,
    },
    /// A request for `"all"` devices resolved to an empty list because the
    /// installed CDI spec declares no devices for the kind.
    #[error("CDI spec for kind '{0}' declares no devices (host has no compatible hardware)")]
    NoDevices(String),
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

    fn fixture_spec_with_hooks() -> &'static str {
        r#"{
            "cdiVersion": "0.6.0",
            "kind": "nvidia.com/gpu",
            "devices": [
                {
                    "name": "0",
                    "containerEdits": {
                        "env": ["NVIDIA_VISIBLE_DEVICES=0"],
                        "deviceNodes": [
                            {"path": "/dev/nvidia0", "type": "c", "major": 195, "minor": 0}
                        ],
                        "hooks": {
                            "createContainer": [{
                                "path": "/usr/bin/nvidia-container-runtime-hook",
                                "args": ["nvidia-container-runtime-hook", "prestart"]
                            }]
                        }
                    }
                },
                {
                    "name": "1",
                    "containerEdits": {
                        "env": ["NVIDIA_VISIBLE_DEVICES=1"],
                        "deviceNodes": [
                            {"path": "/dev/nvidia1", "type": "c", "major": 195, "minor": 1}
                        ]
                    }
                }
            ]
        }"#
    }

    fn registry_with_fixture_dir() -> (tempfile::TempDir, CdiRegistry) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nvidia.json");
        std::fs::write(&path, fixture_spec_with_hooks()).unwrap();
        let registry = CdiRegistry::discover_from(&[dir.path()]);
        (dir, registry)
    }

    #[test]
    fn discover_from_loads_specs() {
        let (_keep, registry) = registry_with_fixture_dir();
        assert_eq!(registry.kinds().count(), 1);
        assert!(registry.get_spec("nvidia.com/gpu").is_some());
    }

    #[test]
    fn discover_from_empty_dir_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let registry = CdiRegistry::discover_from(&[dir.path()]);
        assert!(registry.is_empty());
    }

    #[test]
    fn resolve_for_kind_returns_edits_per_device() {
        let (_keep, registry) = registry_with_fixture_dir();
        let edits = registry
            .resolve_for_kind("nvidia.com/gpu", &["0".to_string()])
            .expect("resolve gpu 0");
        assert_eq!(edits.len(), 1);
        assert!(edits[0].env.iter().any(|e| e == "NVIDIA_VISIBLE_DEVICES=0"));
        assert!(edits[0]
            .device_nodes
            .iter()
            .any(|d| d.path == "/dev/nvidia0"));
        let hooks = edits[0].hooks.as_ref().expect("hooks merged");
        assert_eq!(hooks.create_container.len(), 1);
    }

    #[test]
    fn resolve_for_kind_all_expands_to_every_device() {
        let (_keep, registry) = registry_with_fixture_dir();
        let edits = registry
            .resolve_for_kind("nvidia.com/gpu", &["all".to_string()])
            .expect("resolve all");
        assert_eq!(edits.len(), 2, "should expand to both '0' and '1'");
        let names: Vec<&str> = edits
            .iter()
            .flat_map(|e| e.env.iter())
            .filter(|s| s.starts_with("NVIDIA_VISIBLE_DEVICES="))
            .map(String::as_str)
            .collect();
        assert!(names.contains(&"NVIDIA_VISIBLE_DEVICES=0"));
        assert!(names.contains(&"NVIDIA_VISIBLE_DEVICES=1"));
    }

    #[test]
    fn resolve_for_kind_missing_spec_errors() {
        let registry = CdiRegistry::default();
        let err = registry
            .resolve_for_kind("nvidia.com/gpu", &["0".to_string()])
            .unwrap_err();
        assert!(matches!(err, CdiError::SpecMissing(ref k) if k == "nvidia.com/gpu"));
    }

    #[test]
    fn resolve_for_kind_unknown_device_errors() {
        let (_keep, registry) = registry_with_fixture_dir();
        let err = registry
            .resolve_for_kind("nvidia.com/gpu", &["99".to_string()])
            .unwrap_err();
        assert!(matches!(
            err,
            CdiError::DeviceMissing { ref device, .. } if device == "99"
        ));
    }

    #[test]
    fn vendor_to_cdi_kind_maps_known_vendors() {
        assert_eq!(vendor_to_cdi_kind("nvidia"), Some("nvidia.com/gpu"));
        assert_eq!(vendor_to_cdi_kind("amd"), Some("amd.com/gpu"));
        assert_eq!(vendor_to_cdi_kind("intel"), Some("intel.com/gpu"));
        assert_eq!(vendor_to_cdi_kind("apple"), None);
    }
}
