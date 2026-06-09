//! OCI manifest + config builder helpers for the HCS builder.
//!
//! The real per-instruction build / commit / push path is now owned by
//! [`crate::windows_builder::WindowsBuilder::build_image_for_backend`]; this
//! module retains only the pure JSON-serialization helpers that the
//! `tests/hcs_backend_e2e.rs` integration test exercises directly
//! (`ImageConfigBuilder`, `build_image_config_bytes`, `build_manifest_bytes`,
//! and the OCI media-type constants).

#![cfg(target_os = "windows")]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use zlayer_registry::image_config::{ImageConfig, ImageHealthcheck};
use zlayer_registry::oci_export::{OciDescriptor, OciManifest};

use crate::dockerfile::{HealthcheckInstruction, ShellOrExec};

/// One pre-existing base layer blob, recorded for inclusion in the final
/// OCI manifest.
///
/// Inlined from the (now-deleted) `super::scratch` module so that
/// [`build_manifest_bytes`] still has a typed descriptor for base layers
/// without dragging the rest of the base-chain materialisation code into
/// the dead-shim build path.
#[derive(Debug, Clone)]
pub struct BaseLayerBlob {
    /// Media type (`application/vnd.docker.image.rootfs.foreign.diff.tar.gzip`
    /// or `application/vnd.oci.image.layer.v1.tar+gzip` depending on source).
    pub media_type: String,
    /// Content-addressable digest of the compressed blob (`sha256:...`).
    pub digest: String,
    /// Compressed blob bytes. Required in the manifest unchanged.
    pub bytes: Vec<u8>,
    /// Optional `urls[]` (non-empty for MCR foreign layers).
    pub urls: Vec<String>,
}

/// OCI image-config media type. Public so tests and the backend module can
/// reference the exact string without retyping it.
pub const OCI_IMAGE_CONFIG_MEDIA_TYPE: &str = "application/vnd.oci.image.config.v1+json";

/// OCI image-manifest media type.
pub const OCI_IMAGE_MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";

/// Media type of the new diff layer produced by the HCS builder. OCI spec
/// permits the plain `tar+gzip` type with `os: windows` in the enclosing
/// image config rather than the Docker-era foreign-layer type; we emit the
/// spec-compliant choice for new images we build ourselves.
pub const OCI_WINDOWS_LAYER_MEDIA_TYPE: &str = "application/vnd.oci.image.layer.v1.tar+gzip";

// ---------------------------------------------------------------------------
// ImageConfigBuilder
// ---------------------------------------------------------------------------

/// Accumulator for metadata-only Dockerfile instructions during a build.
///
/// Pairs a [`ImageConfig`] (runtime defaults) with an `os` / `architecture` /
/// `os.version` block (platform identity) that the OCI image-config JSON
/// carries at the top level. Pre-populated with `os: "windows"` /
/// `architecture: "amd64"` since the HCS builder only produces Windows
/// amd64 images today.
#[derive(Debug, Clone)]
pub struct ImageConfigBuilder {
    runtime: ImageConfig,
    os: String,
    architecture: String,
    os_version: Option<String>,
}

impl Default for ImageConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageConfigBuilder {
    /// Create a blank config builder targeting `windows/amd64`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            runtime: ImageConfig::default(),
            os: "windows".to_string(),
            architecture: "amd64".to_string(),
            os_version: None,
        }
    }

    /// Copy runtime defaults (Env/Entrypoint/Cmd/WorkingDir/...) from the
    /// base image config. Applied early, before the Dockerfile's own
    /// instructions have a chance to override.
    pub fn inherit_from_base(&mut self, base: &ImageConfig) {
        // Clone wholesale; subsequent instructions replace fields as needed.
        self.runtime = base.clone();
    }

    /// Record the `os.version` the image must run against. Windows HCS
    /// refuses to run an image built for a different build family, so this
    /// is non-negotiable for Windows.
    pub fn set_os_version(&mut self, version: Option<String>) {
        self.os_version = version;
    }

    /// Append an `ENV KEY=VALUE` pair. If the key already exists it is
    /// replaced (matches Dockerfile semantics).
    pub fn push_env(&mut self, key: &str, value: &str) {
        let entries = self.runtime.env.get_or_insert_with(Vec::new);
        let prefix = format!("{key}=");
        entries.retain(|e| !e.starts_with(&prefix));
        entries.push(format!("{prefix}{value}"));
    }

    /// Set `WorkingDir`.
    pub fn set_working_dir(&mut self, dir: &str) {
        self.runtime.working_dir = Some(dir.to_string());
    }

    /// Return the current `WorkingDir`, if one has been set.
    #[must_use]
    pub fn current_working_dir(&self) -> Option<String> {
        self.runtime.working_dir.clone()
    }

    /// Return the current `User`, if one has been set.
    #[must_use]
    pub fn current_user(&self) -> Option<&str> {
        self.runtime.user.as_deref()
    }

    /// Return the current `Env` list as a `BTreeMap` for
    /// `HcsCreateProcess` process parameters.
    #[must_use]
    pub fn env_map(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        if let Some(ref env) = self.runtime.env {
            for entry in env {
                if let Some((k, v)) = entry.split_once('=') {
                    out.insert(k.to_string(), v.to_string());
                }
            }
        }
        out
    }

    /// Add an `ExposedPorts` entry (`"<port>/<proto>"`).
    pub fn add_exposed_port(&mut self, port: u16, tcp: bool) {
        let map = self
            .runtime
            .exposed_ports
            .get_or_insert_with(Default::default);
        let proto = if tcp { "tcp" } else { "udp" };
        map.insert(
            format!("{port}/{proto}"),
            serde_json::Value::Object(serde_json::Map::new()),
        );
    }

    /// Add a `Labels` entry.
    pub fn add_label(&mut self, key: &str, value: &str) {
        let map = self.runtime.labels.get_or_insert_with(Default::default);
        map.insert(key.to_string(), value.to_string());
    }

    /// Add a `Volumes` entry.
    pub fn add_volume(&mut self, path: &str) {
        let map = self.runtime.volumes.get_or_insert_with(Default::default);
        map.insert(
            path.to_string(),
            serde_json::Value::Object(serde_json::Map::new()),
        );
    }

    /// Set `User`.
    pub fn set_user(&mut self, user: &str) {
        self.runtime.user = Some(user.to_string());
    }

    /// Set `Entrypoint`. Shell-form is wrapped with the translator's active
    /// shell (honors any `SHELL ["…"]` override earlier in the Dockerfile).
    pub fn set_entrypoint(
        &mut self,
        translator: &crate::buildah::DockerfileTranslator,
        cmd: &ShellOrExec,
    ) {
        self.runtime.entrypoint = Some(shellorexec_to_vec(translator, cmd));
    }

    /// Set `Cmd`. Shell-form is wrapped with the translator's active shell.
    pub fn set_cmd(
        &mut self,
        translator: &crate::buildah::DockerfileTranslator,
        cmd: &ShellOrExec,
    ) {
        self.runtime.cmd = Some(shellorexec_to_vec(translator, cmd));
    }

    /// Record a `SHELL ["…"]` Dockerfile instruction into the image config.
    /// This is the metadata-only half — the translator itself already tracks
    /// the override for subsequent RUN/CMD/ENTRYPOINT instructions.
    pub fn set_shell(&mut self, shell: Vec<String>) {
        self.runtime.shell = Some(shell);
    }

    /// Set `StopSignal`.
    pub fn set_stop_signal(&mut self, signal: &str) {
        self.runtime.stop_signal = Some(signal.to_string());
    }

    /// Set `Healthcheck` by converting a Dockerfile instruction. `NONE`
    /// instructions clear the field; check instructions convert into the
    /// `ImageHealthcheck` shape with all durations in nanoseconds.
    pub fn set_healthcheck(&mut self, hc: HealthcheckInstruction) {
        match hc {
            HealthcheckInstruction::None => {
                self.runtime.healthcheck = Some(ImageHealthcheck {
                    test: Some(vec!["NONE".to_string()]),
                    ..Default::default()
                });
            }
            HealthcheckInstruction::Check {
                command,
                interval,
                timeout,
                start_period,
                retries,
                ..
            } => {
                let mut test_vec = Vec::new();
                match &command {
                    ShellOrExec::Shell(s) => {
                        test_vec.push("CMD-SHELL".to_string());
                        test_vec.push(s.clone());
                    }
                    ShellOrExec::Exec(args) => {
                        test_vec.push("CMD".to_string());
                        test_vec.extend(args.iter().cloned());
                    }
                }
                self.runtime.healthcheck = Some(ImageHealthcheck {
                    test: Some(test_vec),
                    #[allow(clippy::cast_possible_truncation)]
                    interval: interval.map(|d| d.as_nanos() as u64),
                    #[allow(clippy::cast_possible_truncation)]
                    timeout: timeout.map(|d| d.as_nanos() as u64),
                    #[allow(clippy::cast_possible_truncation)]
                    start_period: start_period.map(|d| d.as_nanos() as u64),
                    retries,
                });
            }
        }
    }

    /// Borrow the runtime-side config (for tests / diagnostics).
    #[must_use]
    pub fn runtime(&self) -> &ImageConfig {
        &self.runtime
    }
}

// ---------------------------------------------------------------------------
// Image config serialization
// ---------------------------------------------------------------------------

/// Root OCI image-config JSON document. Carries the runtime `config` block
/// plus platform identity (`architecture`, `os`, optional `os.version`) and
/// a `rootfs.diff_ids` array for every layer in the final chain.
#[derive(Debug, Serialize, Deserialize)]
struct OciImageConfig {
    architecture: String,
    os: String,
    #[serde(rename = "os.version", skip_serializing_if = "Option::is_none")]
    os_version: Option<String>,
    config: ImageConfig,
    rootfs: RootFs,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    history: Vec<HistoryEntry>,
    /// Schema-compatibility field: image configs often include a
    /// `"created"` timestamp; we emit a constant sentinel so the same
    /// Dockerfile reliably builds to the same digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    created: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RootFs {
    r#type: String,
    diff_ids: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct HistoryEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    created_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    comment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    empty_layer: Option<bool>,
}

/// Serialize `config` + `diff_ids` into a canonical JSON byte string.
///
/// The `base_diff_ids` list carries the uncompressed-layer digests for each
/// base layer (computed at unpack time); `new_layer_diff_id` is the `diff_id`
/// for the layer this build just produced.
///
/// # Errors
///
/// Returns an error if JSON serialization fails (effectively never — the
/// types only contain plain scalars).
pub fn build_image_config_bytes(
    builder: &ImageConfigBuilder,
    base_diff_ids: &[String],
    new_layer_diff_id: &str,
) -> serde_json::Result<Vec<u8>> {
    let mut diff_ids: Vec<String> = base_diff_ids.to_vec();
    diff_ids.push(new_layer_diff_id.to_string());

    let doc = OciImageConfig {
        architecture: builder.architecture.clone(),
        os: builder.os.clone(),
        os_version: builder.os_version.clone(),
        config: builder.runtime.clone(),
        rootfs: RootFs {
            r#type: "layers".to_string(),
            diff_ids,
        },
        history: Vec::new(),
        // Sentinel epoch timestamp keeps builds deterministic across runs.
        created: Some("1970-01-01T00:00:00Z".to_string()),
    };

    serde_json::to_vec(&doc)
}

// ---------------------------------------------------------------------------
// Manifest construction
// ---------------------------------------------------------------------------

/// Build the OCI image manifest JSON byte string, referencing the config
/// blob plus every layer descriptor (base layers base-first, new diff
/// layer on top).
///
/// # Errors
///
/// Returns an error if JSON serialization fails.
pub fn build_manifest_bytes(
    config_digest: &str,
    config_size: u64,
    base_layers: &[BaseLayerBlob],
    new_layer_digest: &str,
    new_layer_size: u64,
) -> serde_json::Result<Vec<u8>> {
    let mut layers: Vec<OciDescriptor> = base_layers
        .iter()
        .map(|layer| OciDescriptor {
            media_type: layer.media_type.clone(),
            digest: layer.digest.clone(),
            size: layer.bytes.len() as u64,
            urls: if layer.urls.is_empty() {
                None
            } else {
                Some(layer.urls.clone())
            },
            annotations: None,
            platform: None,
        })
        .collect();

    layers.push(OciDescriptor {
        media_type: OCI_WINDOWS_LAYER_MEDIA_TYPE.to_string(),
        digest: new_layer_digest.to_string(),
        size: new_layer_size,
        urls: None,
        annotations: None,
        platform: None,
    });

    let manifest = OciManifest {
        schema_version: 2,
        media_type: Some(OCI_IMAGE_MANIFEST_MEDIA_TYPE.to_string()),
        config: Some(OciDescriptor {
            media_type: OCI_IMAGE_CONFIG_MEDIA_TYPE.to_string(),
            digest: config_digest.to_string(),
            size: config_size,
            urls: None,
            annotations: None,
            platform: None,
        }),
        layers,
        annotations: None,
    };

    serde_json::to_vec(&manifest)
}

// ---------------------------------------------------------------------------
// (Removed) Write-to-disk helpers
// ---------------------------------------------------------------------------
//
// `write_oci_artifacts`, `BuildCommitArtifacts`, `compute_base_diff_ids`,
// `gzip_decode`, and `write_blob` previously lived here. The real builder
// (`crate::windows_builder::WindowsBuilder::build_image_for_backend`) owns
// the OCI layout writer end-to-end now, so those helpers are gone.

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_config() -> ImageConfigBuilder {
        let mut b = ImageConfigBuilder::new();
        b.set_working_dir("C:\\app");
        b.push_env("PATH", "C:\\Windows;C:\\Windows\\System32");
        b.add_label("example", "true");
        b.set_os_version(Some("10.0.20348.2600".to_string()));
        b
    }

    #[test]
    fn image_config_json_has_windows_amd64_identity() {
        let cfg = demo_config();
        let bytes = build_image_config_bytes(&cfg, &[], "sha256:deadbeef").unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["os"], "windows");
        assert_eq!(parsed["architecture"], "amd64");
        assert_eq!(parsed["os.version"], "10.0.20348.2600");
        assert_eq!(parsed["rootfs"]["type"], "layers");
        assert_eq!(parsed["rootfs"]["diff_ids"][0], "sha256:deadbeef");
    }

    #[test]
    fn manifest_uses_standard_oci_media_types() {
        let bytes = build_manifest_bytes(
            "sha256:aaa",
            123,
            &[BaseLayerBlob {
                media_type: "application/vnd.oci.image.layer.v1.tar+gzip".to_string(),
                digest: "sha256:base".to_string(),
                bytes: vec![0; 64],
                urls: vec![],
            }],
            "sha256:new",
            64,
        )
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["schemaVersion"], 2);
        assert_eq!(parsed["mediaType"], OCI_IMAGE_MANIFEST_MEDIA_TYPE);
        assert_eq!(parsed["config"]["mediaType"], OCI_IMAGE_CONFIG_MEDIA_TYPE);
        assert_eq!(parsed["config"]["digest"], "sha256:aaa");
        assert_eq!(
            parsed["layers"][1]["mediaType"],
            OCI_WINDOWS_LAYER_MEDIA_TYPE
        );
        assert_eq!(parsed["layers"][1]["digest"], "sha256:new");
    }

    #[test]
    fn env_map_parses_key_value_list() {
        let mut b = ImageConfigBuilder::new();
        b.push_env("FOO", "bar");
        b.push_env("BAZ", "qux=with=equals");
        let map = b.env_map();
        assert_eq!(map.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(map.get("BAZ").map(String::as_str), Some("qux=with=equals"));
    }

    #[test]
    fn push_env_replaces_existing_key() {
        let mut b = ImageConfigBuilder::new();
        b.push_env("PATH", "/old");
        b.push_env("PATH", "/new");
        let env = b.runtime.env.as_ref().unwrap();
        let path_entries: Vec<_> = env.iter().filter(|e| e.starts_with("PATH=")).collect();
        assert_eq!(path_entries.len(), 1);
        assert_eq!(path_entries[0], "PATH=/new");
    }

    #[test]
    fn shellorexec_to_vec_exec_form_is_verbatim() {
        use crate::backend::ImageOs;
        let t = crate::buildah::DockerfileTranslator::new(ImageOs::Windows);
        let v = shellorexec_to_vec(
            &t,
            &ShellOrExec::Exec(vec![
                "myapp.exe".to_string(),
                "--flag".to_string(),
                "value".to_string(),
            ]),
        );
        assert_eq!(v, vec!["myapp.exe", "--flag", "value"]);
    }

    #[test]
    fn shellorexec_to_vec_shell_form_wraps_in_active_shell() {
        use crate::backend::ImageOs;
        let mut t = crate::buildah::DockerfileTranslator::new(ImageOs::Windows);
        t.set_shell_override(vec!["powershell".to_string(), "-Command".to_string()]);
        let v = shellorexec_to_vec(&t, &ShellOrExec::Shell("Get-Process".to_string()));
        assert_eq!(v, vec!["powershell", "-Command", "Get-Process"]);
    }
}

/// Convert a [`ShellOrExec`] into the `Vec<String>` shape the OCI image
/// config expects for `Cmd` / `Entrypoint`. Shell-form is wrapped with the
/// translator's active shell (honors `SHELL ["…"]` overrides).
fn shellorexec_to_vec(
    translator: &crate::buildah::DockerfileTranslator,
    cmd: &ShellOrExec,
) -> Vec<String> {
    match cmd {
        ShellOrExec::Exec(args) => args.clone(),
        ShellOrExec::Shell(s) => {
            let mut out = translator.active_shell();
            out.push(s.clone());
            out
        }
    }
}
