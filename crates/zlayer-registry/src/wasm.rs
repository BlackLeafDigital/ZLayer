//! WASM artifact detection for OCI images
//!
//! This module provides types and utilities for detecting whether an OCI image
//! is a WASM artifact (WebAssembly module) vs a traditional container image.
//!
//! ## Detection Strategy
//!
//! WASM artifacts can be identified through multiple signals:
//! 1. The `artifact_type` field on the manifest (OCI 1.1+)
//! 2. The `config.media_type` field
//! 3. The layer media types
//!
//! The detection is performed in order of specificity:
//! - If `artifact_type` is set and matches WASM patterns, use that
//! - Otherwise check `config.media_type` for WASM config media type
//! - Finally check layer media types for WASM binary layers

use oci_client::manifest::{OciImageManifest, WASM_CONFIG_MEDIA_TYPE, WASM_LAYER_MEDIA_TYPE};

/// Media type for WASM component (wasip2) artifacts
pub const WASM_COMPONENT_ARTIFACT_TYPE: &str = "application/vnd.wasm.component.v1+wasm";

/// Media type for WASM module (wasip1) artifacts
pub const WASM_MODULE_ARTIFACT_TYPE: &str = "application/vnd.wasm.module.v1+wasm";

/// Alternative config media type used by some WASM tooling
pub const WASM_CONFIG_MEDIA_TYPE_V0: &str = "application/vnd.wasm.config.v0+json";

/// Alternative layer media type (generic application/wasm)
pub const WASM_LAYER_MEDIA_TYPE_GENERIC: &str = "application/wasm";

/// Artifact type detected from OCI manifest
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactType {
    /// Traditional container image (Linux containers, etc.)
    Container,
    /// WebAssembly artifact
    Wasm {
        /// WASI version detected from the artifact
        wasi_version: WasiVersion,
    },
}

impl ArtifactType {
    /// Returns true if this is a WASM artifact
    #[inline]
    pub fn is_wasm(&self) -> bool {
        matches!(self, ArtifactType::Wasm { .. })
    }

    /// Returns true if this is a container image
    #[inline]
    pub fn is_container(&self) -> bool {
        matches!(self, ArtifactType::Container)
    }

    /// Returns the WASI version if this is a WASM artifact
    pub fn wasi_version(&self) -> Option<&WasiVersion> {
        match self {
            ArtifactType::Wasm { wasi_version } => Some(wasi_version),
            ArtifactType::Container => None,
        }
    }
}

impl std::fmt::Display for ArtifactType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArtifactType::Container => write!(f, "container"),
            ArtifactType::Wasm { wasi_version } => write!(f, "wasm ({})", wasi_version),
        }
    }
}

/// WASI version detected from the artifact
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum WasiVersion {
    /// WASI Preview 1 (wasip1) - core module
    Preview1,
    /// WASI Preview 2 (wasip2) - component model
    Preview2,
    /// Unable to determine WASI version
    #[default]
    Unknown,
}

impl WasiVersion {
    /// Returns true if this is wasip1
    #[inline]
    pub fn is_preview1(&self) -> bool {
        matches!(self, WasiVersion::Preview1)
    }

    /// Returns true if this is wasip2 (component model)
    #[inline]
    pub fn is_preview2(&self) -> bool {
        matches!(self, WasiVersion::Preview2)
    }

    /// Returns the expected target triple suffix for this WASI version
    pub fn target_triple_suffix(&self) -> &'static str {
        match self {
            WasiVersion::Preview1 => "wasm32-wasip1",
            WasiVersion::Preview2 => "wasm32-wasip2",
            WasiVersion::Unknown => "wasm32-wasi",
        }
    }
}

impl std::fmt::Display for WasiVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WasiVersion::Preview1 => write!(f, "wasip1"),
            WasiVersion::Preview2 => write!(f, "wasip2"),
            WasiVersion::Unknown => write!(f, "unknown"),
        }
    }
}

/// Detect artifact type from an OCI image manifest
///
/// Uses multiple detection strategies in order of specificity:
/// 1. Check `artifact_type` field for WASM patterns (OCI 1.1+)
/// 2. Check `config.media_type` for WASM config media type
/// 3. Check layer media types for WASM binary layers
///
/// # Examples
///
/// ```
/// use zlayer_registry::wasm::{detect_artifact_type, ArtifactType, WasiVersion};
/// use oci_client::manifest::OciImageManifest;
///
/// // For a container image
/// let manifest = OciImageManifest::default();
/// let artifact_type = detect_artifact_type(&manifest);
/// assert!(artifact_type.is_container());
/// ```
pub fn detect_artifact_type(manifest: &OciImageManifest) -> ArtifactType {
    // Strategy 1: Check artifact_type field (most explicit)
    if let Some(artifact_type) = &manifest.artifact_type {
        if let Some(wasi_version) = detect_wasi_version_from_artifact_type(artifact_type) {
            return ArtifactType::Wasm { wasi_version };
        }
    }

    // Strategy 2: Check config media type
    let config_media_type = &manifest.config.media_type;
    if is_wasm_config_media_type(config_media_type) {
        // Config indicates WASM, try to determine version from layers
        let wasi_version = detect_wasi_version_from_layers(&manifest.layers);
        return ArtifactType::Wasm { wasi_version };
    }

    // Strategy 3: Check layer media types
    if has_wasm_layers(&manifest.layers) {
        let wasi_version = detect_wasi_version_from_layers(&manifest.layers);
        return ArtifactType::Wasm { wasi_version };
    }

    ArtifactType::Container
}

/// Check if a media type indicates a WASM config
fn is_wasm_config_media_type(media_type: &str) -> bool {
    media_type == WASM_CONFIG_MEDIA_TYPE
        || media_type == WASM_CONFIG_MEDIA_TYPE_V0
        || media_type.starts_with("application/vnd.wasm.config.")
}

/// Check if a media type indicates a WASM layer
fn is_wasm_layer_media_type(media_type: &str) -> bool {
    media_type == WASM_LAYER_MEDIA_TYPE
        || media_type == WASM_LAYER_MEDIA_TYPE_GENERIC
        || media_type.starts_with("application/vnd.wasm.content.layer.")
        || media_type.ends_with("+wasm")
}

/// Detect WASI version from artifact type string
fn detect_wasi_version_from_artifact_type(artifact_type: &str) -> Option<WasiVersion> {
    match artifact_type {
        WASM_COMPONENT_ARTIFACT_TYPE => Some(WasiVersion::Preview2),
        WASM_MODULE_ARTIFACT_TYPE => Some(WasiVersion::Preview1),
        // Generic WASM patterns
        s if s.contains("component") && s.contains("wasm") => Some(WasiVersion::Preview2),
        s if s.contains("module") && s.contains("wasm") => Some(WasiVersion::Preview1),
        s if s.contains("wasm") => Some(WasiVersion::Unknown),
        _ => None,
    }
}

/// Check if any layers are WASM layers
fn has_wasm_layers(layers: &[oci_client::manifest::OciDescriptor]) -> bool {
    layers
        .iter()
        .any(|l| is_wasm_layer_media_type(&l.media_type))
}

/// Detect WASI version from layer media types and annotations
fn detect_wasi_version_from_layers(layers: &[oci_client::manifest::OciDescriptor]) -> WasiVersion {
    for layer in layers {
        // Check media type for hints
        let media_type = &layer.media_type;
        if media_type.contains("component") {
            return WasiVersion::Preview2;
        }
        if media_type.contains("module") {
            return WasiVersion::Preview1;
        }

        // Check annotations for additional hints
        if let Some(annotations) = &layer.annotations {
            // Check for component model indicator
            if annotations
                .get("org.opencontainers.image.title")
                .map(|t| t.contains("component"))
                .unwrap_or(false)
            {
                return WasiVersion::Preview2;
            }

            // Check for wit-component annotation
            if annotations.contains_key("org.bytecodealliance.wit-component.version") {
                return WasiVersion::Preview2;
            }
        }
    }

    WasiVersion::Unknown
}

/// Information about a WASM artifact extracted from manifest
#[derive(Debug, Clone)]
pub struct WasmArtifactInfo {
    /// The detected WASI version
    pub wasi_version: WasiVersion,
    /// Digest of the WASM binary layer (if found)
    pub wasm_layer_digest: Option<String>,
    /// Size of the WASM binary in bytes
    pub wasm_size: Option<i64>,
    /// Title/name of the WASM module from annotations
    pub module_name: Option<String>,
}

/// Extract detailed WASM artifact information from a manifest
///
/// Returns `None` if this is not a WASM artifact.
pub fn extract_wasm_info(manifest: &OciImageManifest) -> Option<WasmArtifactInfo> {
    let artifact_type = detect_artifact_type(manifest);
    let wasi_version = artifact_type.wasi_version()?.clone();

    // Find the WASM layer
    let wasm_layer = manifest
        .layers
        .iter()
        .find(|l| is_wasm_layer_media_type(&l.media_type));

    let wasm_layer_digest = wasm_layer.map(|l| l.digest.clone());
    let wasm_size = wasm_layer.map(|l| l.size);

    // Extract module name from annotations
    let module_name = wasm_layer.and_then(|l| {
        l.annotations
            .as_ref()?
            .get("org.opencontainers.image.title")
            .cloned()
    });

    Some(WasmArtifactInfo {
        wasi_version,
        wasm_layer_digest,
        wasm_size,
        module_name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use oci_client::manifest::{OciDescriptor, OciImageManifest};
    use std::collections::BTreeMap;

    fn make_container_manifest() -> OciImageManifest {
        OciImageManifest {
            schema_version: 2,
            media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
            config: OciDescriptor {
                media_type: "application/vnd.oci.image.config.v1+json".to_string(),
                digest: "sha256:abc123".to_string(),
                size: 1234,
                urls: None,
                annotations: None,
            },
            layers: vec![OciDescriptor {
                media_type: "application/vnd.oci.image.layer.v1.tar+gzip".to_string(),
                digest: "sha256:def456".to_string(),
                size: 5678,
                urls: None,
                annotations: None,
            }],
            subject: None,
            artifact_type: None,
            annotations: None,
        }
    }

    fn make_wasm_manifest_with_artifact_type() -> OciImageManifest {
        OciImageManifest {
            schema_version: 2,
            media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
            config: OciDescriptor {
                media_type: WASM_CONFIG_MEDIA_TYPE.to_string(),
                digest: "sha256:abc123".to_string(),
                size: 100,
                urls: None,
                annotations: None,
            },
            layers: vec![OciDescriptor {
                media_type: WASM_LAYER_MEDIA_TYPE.to_string(),
                digest: "sha256:wasm789".to_string(),
                size: 1_615_998,
                urls: None,
                annotations: Some(BTreeMap::from([(
                    "org.opencontainers.image.title".to_string(),
                    "module.wasm".to_string(),
                )])),
            }],
            subject: None,
            artifact_type: Some(WASM_COMPONENT_ARTIFACT_TYPE.to_string()),
            annotations: None,
        }
    }

    fn make_wasm_manifest_config_only() -> OciImageManifest {
        OciImageManifest {
            schema_version: 2,
            media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
            config: OciDescriptor {
                media_type: WASM_CONFIG_MEDIA_TYPE.to_string(),
                digest: "sha256:abc123".to_string(),
                size: 100,
                urls: None,
                annotations: None,
            },
            layers: vec![OciDescriptor {
                media_type: "application/wasm".to_string(),
                digest: "sha256:wasm456".to_string(),
                size: 500_000,
                urls: None,
                annotations: None,
            }],
            subject: None,
            artifact_type: None,
            annotations: None,
        }
    }

    #[test]
    fn test_detect_container() {
        let manifest = make_container_manifest();
        let artifact_type = detect_artifact_type(&manifest);
        assert!(artifact_type.is_container());
        assert!(!artifact_type.is_wasm());
        assert_eq!(artifact_type.to_string(), "container");
    }

    #[test]
    fn test_detect_wasm_with_artifact_type() {
        let manifest = make_wasm_manifest_with_artifact_type();
        let artifact_type = detect_artifact_type(&manifest);
        assert!(artifact_type.is_wasm());
        assert!(!artifact_type.is_container());
        assert_eq!(artifact_type.wasi_version(), Some(&WasiVersion::Preview2));
        assert_eq!(artifact_type.to_string(), "wasm (wasip2)");
    }

    #[test]
    fn test_detect_wasm_config_only() {
        let manifest = make_wasm_manifest_config_only();
        let artifact_type = detect_artifact_type(&manifest);
        assert!(artifact_type.is_wasm());
        // Without artifact_type hint, falls back to Unknown
        assert_eq!(artifact_type.wasi_version(), Some(&WasiVersion::Unknown));
    }

    #[test]
    fn test_extract_wasm_info() {
        let manifest = make_wasm_manifest_with_artifact_type();
        let info = extract_wasm_info(&manifest);
        assert!(info.is_some());

        let info = info.unwrap();
        assert_eq!(info.wasi_version, WasiVersion::Preview2);
        assert_eq!(info.wasm_layer_digest, Some("sha256:wasm789".to_string()));
        assert_eq!(info.wasm_size, Some(1_615_998));
        assert_eq!(info.module_name, Some("module.wasm".to_string()));
    }

    #[test]
    fn test_extract_wasm_info_container() {
        let manifest = make_container_manifest();
        let info = extract_wasm_info(&manifest);
        assert!(info.is_none());
    }

    #[test]
    fn test_wasi_version_display() {
        assert_eq!(WasiVersion::Preview1.to_string(), "wasip1");
        assert_eq!(WasiVersion::Preview2.to_string(), "wasip2");
        assert_eq!(WasiVersion::Unknown.to_string(), "unknown");
    }

    #[test]
    fn test_wasi_version_target_triple() {
        assert_eq!(
            WasiVersion::Preview1.target_triple_suffix(),
            "wasm32-wasip1"
        );
        assert_eq!(
            WasiVersion::Preview2.target_triple_suffix(),
            "wasm32-wasip2"
        );
        assert_eq!(WasiVersion::Unknown.target_triple_suffix(), "wasm32-wasi");
    }

    #[test]
    fn test_is_wasm_config_media_type() {
        assert!(is_wasm_config_media_type(WASM_CONFIG_MEDIA_TYPE));
        assert!(is_wasm_config_media_type(WASM_CONFIG_MEDIA_TYPE_V0));
        assert!(is_wasm_config_media_type(
            "application/vnd.wasm.config.v2+json"
        ));
        assert!(!is_wasm_config_media_type(
            "application/vnd.oci.image.config.v1+json"
        ));
    }

    #[test]
    fn test_is_wasm_layer_media_type() {
        assert!(is_wasm_layer_media_type(WASM_LAYER_MEDIA_TYPE));
        assert!(is_wasm_layer_media_type(WASM_LAYER_MEDIA_TYPE_GENERIC));
        assert!(is_wasm_layer_media_type(
            "application/vnd.wasm.content.layer.v2+wasm"
        ));
        assert!(is_wasm_layer_media_type("application/vnd.custom.type+wasm"));
        assert!(!is_wasm_layer_media_type(
            "application/vnd.oci.image.layer.v1.tar+gzip"
        ));
    }

    #[test]
    fn test_detect_wasi_version_from_artifact_type() {
        assert_eq!(
            detect_wasi_version_from_artifact_type(WASM_COMPONENT_ARTIFACT_TYPE),
            Some(WasiVersion::Preview2)
        );
        assert_eq!(
            detect_wasi_version_from_artifact_type(WASM_MODULE_ARTIFACT_TYPE),
            Some(WasiVersion::Preview1)
        );
        assert_eq!(
            detect_wasi_version_from_artifact_type("application/vnd.custom.wasm"),
            Some(WasiVersion::Unknown)
        );
        assert_eq!(
            detect_wasi_version_from_artifact_type("application/json"),
            None
        );
    }
}
