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
use thiserror::Error;

/// Media type for WASM component (wasip2) artifacts
pub const WASM_COMPONENT_ARTIFACT_TYPE: &str = "application/vnd.wasm.component.v1+wasm";

/// Media type for WASM module (wasip1) artifacts
pub const WASM_MODULE_ARTIFACT_TYPE: &str = "application/vnd.wasm.module.v1+wasm";

/// Alternative config media type used by some WASM tooling
pub const WASM_CONFIG_MEDIA_TYPE_V0: &str = "application/vnd.wasm.config.v0+json";

/// Alternative layer media type (generic application/wasm)
pub const WASM_LAYER_MEDIA_TYPE_GENERIC: &str = "application/wasm";

/// WASM binary magic bytes: `\0asm` (0x00, 0x61, 0x73, 0x6d)
pub const WASM_MAGIC_BYTES: [u8; 4] = [0x00, 0x61, 0x73, 0x6d];

/// Minimum size of a valid WASM binary (magic + version = 8 bytes)
pub const WASM_MIN_SIZE: usize = 8;

/// Error types for WASM binary analysis
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum WasmError {
    /// Binary does not start with WASM magic bytes
    #[error("invalid WASM magic bytes: expected \\0asm (0x00, 0x61, 0x73, 0x6d)")]
    InvalidMagic,

    /// Binary is too short to be a valid WASM module
    #[error("WASM binary too short: expected at least {WASM_MIN_SIZE} bytes")]
    TooShort,

    /// Invalid or malformed WASM structure
    #[error("invalid WASM structure: {reason}")]
    InvalidStructure { reason: String },
}

/// Information extracted from a WASM binary
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasmBinaryInfo {
    /// Detected WASI version based on binary analysis
    pub wasi_version: WasiVersion,
    /// Whether this is a component (WASIp2) vs a core module (WASIp1)
    pub is_component: bool,
    /// WASM binary format version (typically 1)
    pub binary_version: u32,
    /// Size of the binary in bytes
    pub size: usize,
}

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

    /// Returns the OCI artifact type for this WASI version
    ///
    /// - WASIp1 modules use `application/vnd.wasm.module.v1+wasm`
    /// - WASIp2 components use `application/vnd.wasm.component.v1+wasm`
    pub fn artifact_type(&self) -> &'static str {
        match self {
            WasiVersion::Preview1 | WasiVersion::Unknown => WASM_MODULE_ARTIFACT_TYPE,
            WasiVersion::Preview2 => WASM_COMPONENT_ARTIFACT_TYPE,
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

// =============================================================================
// WASM Binary Analysis Functions
// =============================================================================

/// Validate that a byte slice starts with the WASM magic bytes
///
/// WASM binaries must start with the magic bytes `\0asm` (0x00, 0x61, 0x73, 0x6d).
///
/// # Arguments
///
/// * `bytes` - The binary data to validate
///
/// # Returns
///
/// `true` if the bytes start with valid WASM magic, `false` otherwise.
///
/// # Examples
///
/// ```
/// use zlayer_registry::wasm::validate_wasm_magic;
///
/// // Valid WASM magic
/// let wasm_bytes = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
/// assert!(validate_wasm_magic(&wasm_bytes));
///
/// // Invalid magic
/// let invalid_bytes = [0x7f, 0x45, 0x4c, 0x46]; // ELF header
/// assert!(!validate_wasm_magic(&invalid_bytes));
///
/// // Too short
/// let short_bytes = [0x00, 0x61];
/// assert!(!validate_wasm_magic(&short_bytes));
/// ```
#[inline]
pub fn validate_wasm_magic(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes[0..4] == WASM_MAGIC_BYTES
}

/// Detect WASI version by analyzing the WASM binary structure
///
/// This function examines the binary to determine if it's a:
/// - **WASIp2 Component**: Uses the component model with a component section (layer type 0x00
///   after the 8-byte preamble indicates a component)
/// - **WASIp1 Module**: A core WebAssembly module with standard section types (0x01-0x0c)
///
/// # Binary Format Reference
///
/// Both modules and components start with:
/// - Bytes 0-3: Magic bytes `\0asm`
/// - Bytes 4-7: Version (little-endian u32, typically 1 for modules, varies for components)
///
/// After the preamble:
/// - **Components (WASIp2)**: First section has type 0x00 (component section)
/// - **Modules (WASIp1)**: Sections have types 0x01-0x0c (custom, type, import, etc.)
///
/// # Arguments
///
/// * `bytes` - The WASM binary data
///
/// # Returns
///
/// The detected `WasiVersion`:
/// - `Preview2` for component model binaries
/// - `Preview1` for core module binaries
/// - `Unknown` if the binary is too short or structure is unclear
///
/// # Examples
///
/// ```
/// use zlayer_registry::wasm::{detect_wasm_version_from_binary, WasiVersion};
///
/// // Core module with type section (0x01)
/// let module_bytes = [
///     0x00, 0x61, 0x73, 0x6d, // magic
///     0x01, 0x00, 0x00, 0x00, // version 1
///     0x01,                   // type section
/// ];
/// assert_eq!(detect_wasm_version_from_binary(&module_bytes), WasiVersion::Preview1);
/// ```
pub fn detect_wasm_version_from_binary(bytes: &[u8]) -> WasiVersion {
    // Need at least magic (4) + version (4) + section type (1) = 9 bytes
    if bytes.len() < 9 {
        return WasiVersion::Unknown;
    }

    // Validate magic bytes first
    if !validate_wasm_magic(bytes) {
        return WasiVersion::Unknown;
    }

    // The section type byte comes after the 8-byte preamble
    let first_section_type = bytes[8];

    // Component model binaries have a component section with type 0x00
    // Core modules have section types 0x01-0x0c:
    //   0x00 = custom section (also valid in modules, but components use it differently)
    //   0x01 = type section
    //   0x02 = import section
    //   0x03 = function section
    //   0x04 = table section
    //   0x05 = memory section
    //   0x06 = global section
    //   0x07 = export section
    //   0x08 = start section
    //   0x09 = element section
    //   0x0a = code section
    //   0x0b = data section
    //   0x0c = data count section

    // Check the binary version field to help distinguish
    // Components typically use version 0x0d (13) or higher in the layer byte
    // But the more reliable check is the section structure

    // For a component, the first byte after preamble is typically 0x00 followed by
    // component-specific encoding. For modules, first section is usually 0x01 (type)
    // or 0x00 (custom section like name section).

    // A more robust heuristic: check version bytes
    // Core modules use version 1 (0x01, 0x00, 0x00, 0x00)
    // Components use different version encoding
    let version = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);

    // Component model uses version 0x0d000000 or similar high values
    // Core WASM modules use version 0x00000001
    if version == 1 {
        // This is a core module (WASIp1)
        // Additional validation: check that section type is valid for core module
        if first_section_type <= 0x0c {
            return WasiVersion::Preview1;
        }
    } else if version >= 0x0d {
        // High version number indicates component model
        return WasiVersion::Preview2;
    }

    // Fallback heuristic: if first section type is 0x00 and version != 1, likely component
    if first_section_type == 0x00 && version != 1 {
        return WasiVersion::Preview2;
    }

    WasiVersion::Unknown
}

/// Extract comprehensive information from a WASM binary
///
/// This function validates the binary structure and extracts metadata including
/// the WASI version, whether it's a component, the binary format version, and size.
///
/// # Arguments
///
/// * `bytes` - The WASM binary data
///
/// # Returns
///
/// `Ok(WasmBinaryInfo)` with extracted information, or `Err(WasmError)` if the binary
/// is invalid.
///
/// # Errors
///
/// - `WasmError::TooShort` - Binary is less than 8 bytes
/// - `WasmError::InvalidMagic` - Binary doesn't start with WASM magic bytes
/// - `WasmError::InvalidStructure` - Binary has invalid version or structure
///
/// # Examples
///
/// ```
/// use zlayer_registry::wasm::{extract_wasm_binary_info, WasiVersion};
///
/// let module_bytes = [
///     0x00, 0x61, 0x73, 0x6d, // magic
///     0x01, 0x00, 0x00, 0x00, // version 1
///     0x01,                   // type section
/// ];
///
/// let info = extract_wasm_binary_info(&module_bytes).unwrap();
/// assert_eq!(info.wasi_version, WasiVersion::Preview1);
/// assert!(!info.is_component);
/// assert_eq!(info.binary_version, 1);
/// assert_eq!(info.size, 9);
/// ```
pub fn extract_wasm_binary_info(bytes: &[u8]) -> Result<WasmBinaryInfo, WasmError> {
    // Check minimum size
    if bytes.len() < WASM_MIN_SIZE {
        return Err(WasmError::TooShort);
    }

    // Validate magic bytes
    if !validate_wasm_magic(bytes) {
        return Err(WasmError::InvalidMagic);
    }

    // Extract version (bytes 4-7, little-endian u32)
    let binary_version = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);

    // Validate version is reasonable (known versions are 1 for modules, higher for components)
    // Version 0 is invalid
    if binary_version == 0 {
        return Err(WasmError::InvalidStructure {
            reason: "WASM version cannot be 0".to_string(),
        });
    }

    // Detect WASI version
    let wasi_version = detect_wasm_version_from_binary(bytes);

    // Determine if this is a component based on version and detected WASI version
    let is_component = wasi_version == WasiVersion::Preview2;

    Ok(WasmBinaryInfo {
        wasi_version,
        is_component,
        binary_version,
        size: bytes.len(),
    })
}

// =============================================================================
// OCI Manifest Detection Functions
// =============================================================================

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

    // =========================================================================
    // WASM Binary Analysis Tests
    // =========================================================================

    /// Helper to create a valid WASM module header (version 1)
    fn make_wasm_module_bytes() -> Vec<u8> {
        vec![
            0x00, 0x61, 0x73, 0x6d, // magic: \0asm
            0x01, 0x00, 0x00, 0x00, // version: 1 (little-endian)
            0x01, // type section (0x01)
            0x04, // section size: 4 bytes
            0x01, // 1 type
            0x60, // func type
            0x00, // 0 params
            0x00, // 0 results
        ]
    }

    /// Helper to create a WASM component header (higher version)
    fn make_wasm_component_bytes() -> Vec<u8> {
        vec![
            0x00, 0x61, 0x73, 0x6d, // magic: \0asm
            0x0d, 0x00, 0x00, 0x00, // version: 13 (component model)
            0x00, // component section type
            0x08, // section size
            0x00, 0x01, 0x02, 0x03, // some component data
            0x04, 0x05, 0x06, 0x07,
        ]
    }

    #[test]
    fn test_validate_wasm_magic_valid() {
        let bytes = make_wasm_module_bytes();
        assert!(validate_wasm_magic(&bytes));
    }

    #[test]
    fn test_validate_wasm_magic_component() {
        let bytes = make_wasm_component_bytes();
        assert!(validate_wasm_magic(&bytes));
    }

    #[test]
    fn test_validate_wasm_magic_invalid() {
        // ELF header
        let elf_bytes = [0x7f, 0x45, 0x4c, 0x46, 0x02, 0x01, 0x01, 0x00];
        assert!(!validate_wasm_magic(&elf_bytes));

        // Random garbage
        let garbage = [0xde, 0xad, 0xbe, 0xef];
        assert!(!validate_wasm_magic(&garbage));

        // Almost correct (wrong first byte)
        let almost = [0x01, 0x61, 0x73, 0x6d];
        assert!(!validate_wasm_magic(&almost));
    }

    #[test]
    fn test_validate_wasm_magic_too_short() {
        let short = [0x00, 0x61, 0x73];
        assert!(!validate_wasm_magic(&short));

        let empty: [u8; 0] = [];
        assert!(!validate_wasm_magic(&empty));
    }

    #[test]
    fn test_validate_wasm_magic_exact_length() {
        // Exactly 4 bytes with valid magic should pass
        let exact = [0x00, 0x61, 0x73, 0x6d];
        assert!(validate_wasm_magic(&exact));
    }

    #[test]
    fn test_detect_wasm_version_from_binary_module() {
        let bytes = make_wasm_module_bytes();
        let version = detect_wasm_version_from_binary(&bytes);
        assert_eq!(version, WasiVersion::Preview1);
    }

    #[test]
    fn test_detect_wasm_version_from_binary_component() {
        let bytes = make_wasm_component_bytes();
        let version = detect_wasm_version_from_binary(&bytes);
        assert_eq!(version, WasiVersion::Preview2);
    }

    #[test]
    fn test_detect_wasm_version_from_binary_too_short() {
        // Only 8 bytes (no section type byte)
        let bytes = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        let version = detect_wasm_version_from_binary(&bytes);
        assert_eq!(version, WasiVersion::Unknown);
    }

    #[test]
    fn test_detect_wasm_version_from_binary_invalid_magic() {
        let bytes = [0x7f, 0x45, 0x4c, 0x46, 0x01, 0x00, 0x00, 0x00, 0x01];
        let version = detect_wasm_version_from_binary(&bytes);
        assert_eq!(version, WasiVersion::Unknown);
    }

    #[test]
    fn test_detect_wasm_version_custom_section() {
        // Module with custom section (0x00) but version 1 should still detect as module
        let bytes = vec![
            0x00, 0x61, 0x73, 0x6d, // magic
            0x01, 0x00, 0x00, 0x00, // version 1
            0x00, // custom section (valid in both, but version disambiguates)
            0x04, 0x6e, 0x61, 0x6d, 0x65, // "name" section
        ];
        let version = detect_wasm_version_from_binary(&bytes);
        // Version 1 with section type <= 0x0c should be Preview1
        assert_eq!(version, WasiVersion::Preview1);
    }

    #[test]
    fn test_extract_wasm_binary_info_module() {
        let bytes = make_wasm_module_bytes();
        let info = extract_wasm_binary_info(&bytes).unwrap();

        assert_eq!(info.wasi_version, WasiVersion::Preview1);
        assert!(!info.is_component);
        assert_eq!(info.binary_version, 1);
        assert_eq!(info.size, bytes.len());
    }

    #[test]
    fn test_extract_wasm_binary_info_component() {
        let bytes = make_wasm_component_bytes();
        let info = extract_wasm_binary_info(&bytes).unwrap();

        assert_eq!(info.wasi_version, WasiVersion::Preview2);
        assert!(info.is_component);
        assert_eq!(info.binary_version, 13);
        assert_eq!(info.size, bytes.len());
    }

    #[test]
    fn test_extract_wasm_binary_info_too_short() {
        let bytes = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00]; // 7 bytes
        let result = extract_wasm_binary_info(&bytes);

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), WasmError::TooShort);
    }

    #[test]
    fn test_extract_wasm_binary_info_invalid_magic() {
        let bytes = [0x7f, 0x45, 0x4c, 0x46, 0x01, 0x00, 0x00, 0x00];
        let result = extract_wasm_binary_info(&bytes);

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), WasmError::InvalidMagic);
    }

    #[test]
    fn test_extract_wasm_binary_info_version_zero() {
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, // magic
            0x00, 0x00, 0x00, 0x00, // version 0 (invalid)
        ];
        let result = extract_wasm_binary_info(&bytes);

        assert!(result.is_err());
        match result.unwrap_err() {
            WasmError::InvalidStructure { reason } => {
                assert!(reason.contains("version"));
            }
            other => panic!("Expected InvalidStructure, got {:?}", other),
        }
    }

    #[test]
    fn test_wasm_error_display() {
        assert_eq!(
            WasmError::InvalidMagic.to_string(),
            "invalid WASM magic bytes: expected \\0asm (0x00, 0x61, 0x73, 0x6d)"
        );

        assert_eq!(
            WasmError::TooShort.to_string(),
            "WASM binary too short: expected at least 8 bytes"
        );

        let structure_error = WasmError::InvalidStructure {
            reason: "test reason".to_string(),
        };
        assert_eq!(
            structure_error.to_string(),
            "invalid WASM structure: test reason"
        );
    }

    #[test]
    fn test_wasm_binary_info_equality() {
        let info1 = WasmBinaryInfo {
            wasi_version: WasiVersion::Preview1,
            is_component: false,
            binary_version: 1,
            size: 100,
        };

        let info2 = WasmBinaryInfo {
            wasi_version: WasiVersion::Preview1,
            is_component: false,
            binary_version: 1,
            size: 100,
        };

        let info3 = WasmBinaryInfo {
            wasi_version: WasiVersion::Preview2,
            is_component: true,
            binary_version: 13,
            size: 200,
        };

        assert_eq!(info1, info2);
        assert_ne!(info1, info3);
    }

    #[test]
    fn test_wasm_error_equality() {
        assert_eq!(WasmError::InvalidMagic, WasmError::InvalidMagic);
        assert_eq!(WasmError::TooShort, WasmError::TooShort);
        assert_ne!(WasmError::InvalidMagic, WasmError::TooShort);

        let err1 = WasmError::InvalidStructure {
            reason: "test".to_string(),
        };
        let err2 = WasmError::InvalidStructure {
            reason: "test".to_string(),
        };
        let err3 = WasmError::InvalidStructure {
            reason: "other".to_string(),
        };

        assert_eq!(err1, err2);
        assert_ne!(err1, err3);
    }

    #[test]
    fn test_wasm_constants() {
        // Verify magic bytes are correct
        assert_eq!(WASM_MAGIC_BYTES, [0x00, 0x61, 0x73, 0x6d]);
        assert_eq!(&WASM_MAGIC_BYTES, b"\0asm");

        // Verify minimum size
        assert_eq!(WASM_MIN_SIZE, 8);
    }

    // =========================================================================
    // Additional validate_wasm_magic tests
    // =========================================================================

    #[test]
    fn test_validate_wasm_magic_empty_bytes() {
        let empty: &[u8] = &[];
        assert!(!validate_wasm_magic(empty));
    }

    #[test]
    fn test_validate_wasm_magic_single_byte() {
        let single = [0x00];
        assert!(!validate_wasm_magic(&single));
    }

    #[test]
    fn test_validate_wasm_magic_two_bytes() {
        let two = [0x00, 0x61];
        assert!(!validate_wasm_magic(&two));
    }

    #[test]
    fn test_validate_wasm_magic_three_bytes() {
        let three = [0x00, 0x61, 0x73];
        assert!(!validate_wasm_magic(&three));
    }

    #[test]
    fn test_validate_wasm_magic_with_trailing_garbage() {
        // Valid magic followed by garbage should still pass (we only check first 4 bytes)
        let bytes = [0x00, 0x61, 0x73, 0x6d, 0xff, 0xff, 0xff, 0xff];
        assert!(validate_wasm_magic(&bytes));
    }

    #[test]
    fn test_validate_wasm_magic_all_zeros() {
        let zeros = [0x00, 0x00, 0x00, 0x00];
        assert!(!validate_wasm_magic(&zeros));
    }

    #[test]
    fn test_validate_wasm_magic_off_by_one_each_byte() {
        // Test each magic byte being wrong
        let wrong_byte_0 = [0x01, 0x61, 0x73, 0x6d];
        assert!(!validate_wasm_magic(&wrong_byte_0));

        let wrong_byte_1 = [0x00, 0x62, 0x73, 0x6d];
        assert!(!validate_wasm_magic(&wrong_byte_1));

        let wrong_byte_2 = [0x00, 0x61, 0x74, 0x6d];
        assert!(!validate_wasm_magic(&wrong_byte_2));

        let wrong_byte_3 = [0x00, 0x61, 0x73, 0x6e];
        assert!(!validate_wasm_magic(&wrong_byte_3));
    }

    // =========================================================================
    // Additional detect_wasm_version_from_binary tests
    // =========================================================================

    #[test]
    fn test_detect_wasm_version_minimal_valid_module() {
        // Minimal valid module: magic + version + type section
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, // magic
            0x01, 0x00, 0x00, 0x00, // version 1
            0x01, // type section marker
        ];
        let version = detect_wasm_version_from_binary(&bytes);
        assert_eq!(version, WasiVersion::Preview1);
    }

    #[test]
    fn test_detect_wasm_version_minimal_valid_component() {
        // Minimal valid component: magic + version 13 + component section
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, // magic
            0x0d, 0x00, 0x00, 0x00, // version 13
            0x00, // component section type
        ];
        let version = detect_wasm_version_from_binary(&bytes);
        assert_eq!(version, WasiVersion::Preview2);
    }

    #[test]
    fn test_detect_wasm_version_module_with_import_section() {
        // Module with import section (0x02)
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, // magic
            0x01, 0x00, 0x00, 0x00, // version 1
            0x02, // import section
        ];
        let version = detect_wasm_version_from_binary(&bytes);
        assert_eq!(version, WasiVersion::Preview1);
    }

    #[test]
    fn test_detect_wasm_version_module_with_memory_section() {
        // Module with memory section (0x05)
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, // magic
            0x01, 0x00, 0x00, 0x00, // version 1
            0x05, // memory section
        ];
        let version = detect_wasm_version_from_binary(&bytes);
        assert_eq!(version, WasiVersion::Preview1);
    }

    #[test]
    fn test_detect_wasm_version_module_with_code_section() {
        // Module with code section (0x0a)
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, // magic
            0x01, 0x00, 0x00, 0x00, // version 1
            0x0a, // code section
        ];
        let version = detect_wasm_version_from_binary(&bytes);
        assert_eq!(version, WasiVersion::Preview1);
    }

    #[test]
    fn test_detect_wasm_version_module_with_data_count_section() {
        // Module with data count section (0x0c) - highest valid core module section
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, // magic
            0x01, 0x00, 0x00, 0x00, // version 1
            0x0c, // data count section
        ];
        let version = detect_wasm_version_from_binary(&bytes);
        assert_eq!(version, WasiVersion::Preview1);
    }

    #[test]
    fn test_detect_wasm_version_corrupt_section_type() {
        // Version 1 with invalid section type (> 0x0c)
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, // magic
            0x01, 0x00, 0x00, 0x00, // version 1
            0x0f, // invalid section type for core module
        ];
        let version = detect_wasm_version_from_binary(&bytes);
        // Section type > 0x0c with version 1 should return Unknown
        assert_eq!(version, WasiVersion::Unknown);
    }

    #[test]
    fn test_detect_wasm_version_version_2() {
        // Hypothetical version 2 module (should return Unknown as we don't recognize it)
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, // magic
            0x02, 0x00, 0x00, 0x00, // version 2
            0x01, // type section
        ];
        let version = detect_wasm_version_from_binary(&bytes);
        // Version 2 is not recognized
        assert_eq!(version, WasiVersion::Unknown);
    }

    #[test]
    fn test_detect_wasm_version_high_version_numbers() {
        // Test various high version numbers that indicate components
        for v in [0x0d, 0x0e, 0x0f, 0x10, 0x20, 0xff] {
            let bytes = [
                0x00, 0x61, 0x73, 0x6d, // magic
                v, 0x00, 0x00, 0x00, // high version
                0x00, // some section
            ];
            let version = detect_wasm_version_from_binary(&bytes);
            assert_eq!(
                version,
                WasiVersion::Preview2,
                "Expected Preview2 for version {}",
                v
            );
        }
    }

    #[test]
    fn test_detect_wasm_version_exactly_8_bytes() {
        // Exactly 8 bytes (no section type) should return Unknown
        let bytes = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        let version = detect_wasm_version_from_binary(&bytes);
        assert_eq!(version, WasiVersion::Unknown);
    }

    #[test]
    fn test_detect_wasm_version_empty() {
        let empty: &[u8] = &[];
        let version = detect_wasm_version_from_binary(empty);
        assert_eq!(version, WasiVersion::Unknown);
    }

    // =========================================================================
    // Additional extract_wasm_binary_info tests
    // =========================================================================

    #[test]
    fn test_extract_wasm_binary_info_minimal_valid() {
        // Exactly 8 bytes with valid magic and version 1
        let bytes = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        let info = extract_wasm_binary_info(&bytes).unwrap();

        assert_eq!(info.binary_version, 1);
        assert_eq!(info.size, 8);
        // Without section type byte, version detection returns Unknown
        assert_eq!(info.wasi_version, WasiVersion::Unknown);
        assert!(!info.is_component);
    }

    #[test]
    fn test_extract_wasm_binary_info_large_binary() {
        // Simulate a larger binary
        let mut bytes = vec![
            0x00, 0x61, 0x73, 0x6d, // magic
            0x01, 0x00, 0x00, 0x00, // version 1
            0x01, // type section
        ];
        // Add some padding to simulate a larger file
        bytes.extend(vec![0x00; 10000]);

        let info = extract_wasm_binary_info(&bytes).unwrap();
        assert_eq!(info.size, 10009);
        assert_eq!(info.wasi_version, WasiVersion::Preview1);
    }

    #[test]
    fn test_extract_wasm_binary_info_empty() {
        let empty: &[u8] = &[];
        let result = extract_wasm_binary_info(empty);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), WasmError::TooShort);
    }

    #[test]
    fn test_extract_wasm_binary_info_truncated_at_various_lengths() {
        // Test truncation at each byte position up to 7
        for len in 0..WASM_MIN_SIZE {
            let bytes = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
            let truncated = &bytes[..len];
            let result = extract_wasm_binary_info(truncated);
            assert!(
                result.is_err(),
                "Expected error for {} bytes, got {:?}",
                len,
                result
            );
            assert_eq!(
                result.unwrap_err(),
                WasmError::TooShort,
                "Expected TooShort for {} bytes",
                len
            );
        }
    }

    #[test]
    fn test_extract_wasm_binary_info_high_binary_version() {
        // High binary version should work and indicate component
        // Little-endian: 0x0d, 0x00, 0x01, 0x00 = 0x00_01_00_0d = 65549
        let bytes = [
            0x00, 0x61, 0x73, 0x6d, // magic
            0x0d, 0x00, 0x01, 0x00, // version 65549 in little-endian
            0x00, // section
        ];
        let info = extract_wasm_binary_info(&bytes).unwrap();
        assert_eq!(info.binary_version, 65549);
        assert!(info.is_component);
    }

    #[test]
    fn test_extract_wasm_binary_info_fields_correct_for_module() {
        let bytes = make_wasm_module_bytes();
        let info = extract_wasm_binary_info(&bytes).unwrap();

        // Verify all fields
        assert_eq!(info.wasi_version, WasiVersion::Preview1);
        assert!(!info.is_component);
        assert_eq!(info.binary_version, 1);
        assert_eq!(info.size, bytes.len());

        // Verify is_component matches wasi_version
        assert_eq!(
            info.is_component,
            info.wasi_version == WasiVersion::Preview2
        );
    }

    #[test]
    fn test_extract_wasm_binary_info_fields_correct_for_component() {
        let bytes = make_wasm_component_bytes();
        let info = extract_wasm_binary_info(&bytes).unwrap();

        // Verify all fields
        assert_eq!(info.wasi_version, WasiVersion::Preview2);
        assert!(info.is_component);
        assert_eq!(info.binary_version, 13);
        assert_eq!(info.size, bytes.len());

        // Verify is_component matches wasi_version
        assert_eq!(
            info.is_component,
            info.wasi_version == WasiVersion::Preview2
        );
    }

    // =========================================================================
    // Additional WasmError tests
    // =========================================================================

    #[test]
    fn test_wasm_error_debug_formatting() {
        let magic_err = WasmError::InvalidMagic;
        let debug_str = format!("{:?}", magic_err);
        assert!(debug_str.contains("InvalidMagic"));

        let short_err = WasmError::TooShort;
        let debug_str = format!("{:?}", short_err);
        assert!(debug_str.contains("TooShort"));

        let struct_err = WasmError::InvalidStructure {
            reason: "custom reason".to_string(),
        };
        let debug_str = format!("{:?}", struct_err);
        assert!(debug_str.contains("InvalidStructure"));
        assert!(debug_str.contains("custom reason"));
    }

    #[test]
    fn test_wasm_error_clone() {
        let err1 = WasmError::InvalidMagic;
        let err2 = err1.clone();
        assert_eq!(err1, err2);

        let err3 = WasmError::InvalidStructure {
            reason: "test".to_string(),
        };
        let err4 = err3.clone();
        assert_eq!(err3, err4);
    }

    #[test]
    fn test_wasm_error_invalid_structure_empty_reason() {
        let err = WasmError::InvalidStructure {
            reason: String::new(),
        };
        assert_eq!(err.to_string(), "invalid WASM structure: ");
    }

    #[test]
    fn test_wasm_error_invalid_structure_long_reason() {
        let long_reason = "a".repeat(1000);
        let err = WasmError::InvalidStructure {
            reason: long_reason.clone(),
        };
        assert!(err.to_string().contains(&long_reason));
    }

    // =========================================================================
    // Additional WasmBinaryInfo tests
    // =========================================================================

    #[test]
    fn test_wasm_binary_info_debug_formatting() {
        let info = WasmBinaryInfo {
            wasi_version: WasiVersion::Preview1,
            is_component: false,
            binary_version: 1,
            size: 1024,
        };

        let debug_str = format!("{:?}", info);
        assert!(debug_str.contains("WasmBinaryInfo"));
        assert!(debug_str.contains("Preview1"));
        assert!(debug_str.contains("is_component"));
        assert!(debug_str.contains("binary_version"));
        assert!(debug_str.contains("size"));
    }

    #[test]
    fn test_wasm_binary_info_clone() {
        let info1 = WasmBinaryInfo {
            wasi_version: WasiVersion::Preview2,
            is_component: true,
            binary_version: 13,
            size: 2048,
        };

        let info2 = info1.clone();
        assert_eq!(info1, info2);
        assert_eq!(info2.wasi_version, WasiVersion::Preview2);
        assert!(info2.is_component);
        assert_eq!(info2.binary_version, 13);
        assert_eq!(info2.size, 2048);
    }

    #[test]
    fn test_wasm_binary_info_inequality_each_field() {
        let base = WasmBinaryInfo {
            wasi_version: WasiVersion::Preview1,
            is_component: false,
            binary_version: 1,
            size: 100,
        };

        // Different wasi_version
        let diff_version = WasmBinaryInfo {
            wasi_version: WasiVersion::Preview2,
            ..base.clone()
        };
        assert_ne!(base, diff_version);

        // Different is_component
        let diff_component = WasmBinaryInfo {
            is_component: true,
            ..base.clone()
        };
        assert_ne!(base, diff_component);

        // Different binary_version
        let diff_binary = WasmBinaryInfo {
            binary_version: 2,
            ..base.clone()
        };
        assert_ne!(base, diff_binary);

        // Different size
        let diff_size = WasmBinaryInfo {
            size: 200,
            ..base.clone()
        };
        assert_ne!(base, diff_size);
    }

    // =========================================================================
    // WasiVersion additional tests
    // =========================================================================

    #[test]
    fn test_wasi_version_default() {
        let default: WasiVersion = Default::default();
        assert_eq!(default, WasiVersion::Unknown);
    }

    #[test]
    fn test_wasi_version_clone() {
        let v1 = WasiVersion::Preview1;
        let v2 = v1.clone();
        assert_eq!(v1, v2);

        let v3 = WasiVersion::Preview2;
        let v4 = v3.clone();
        assert_eq!(v3, v4);
    }

    #[test]
    fn test_wasi_version_debug_formatting() {
        assert!(format!("{:?}", WasiVersion::Preview1).contains("Preview1"));
        assert!(format!("{:?}", WasiVersion::Preview2).contains("Preview2"));
        assert!(format!("{:?}", WasiVersion::Unknown).contains("Unknown"));
    }

    #[test]
    fn test_wasi_version_is_methods() {
        let p1 = WasiVersion::Preview1;
        assert!(p1.is_preview1());
        assert!(!p1.is_preview2());

        let p2 = WasiVersion::Preview2;
        assert!(!p2.is_preview1());
        assert!(p2.is_preview2());

        let unknown = WasiVersion::Unknown;
        assert!(!unknown.is_preview1());
        assert!(!unknown.is_preview2());
    }

    // =========================================================================
    // ArtifactType additional tests
    // =========================================================================

    #[test]
    fn test_artifact_type_debug_formatting() {
        let container = ArtifactType::Container;
        assert!(format!("{:?}", container).contains("Container"));

        let wasm = ArtifactType::Wasm {
            wasi_version: WasiVersion::Preview1,
        };
        assert!(format!("{:?}", wasm).contains("Wasm"));
        assert!(format!("{:?}", wasm).contains("Preview1"));
    }

    #[test]
    fn test_artifact_type_clone() {
        let container = ArtifactType::Container;
        let container_clone = container.clone();
        assert_eq!(container, container_clone);

        let wasm = ArtifactType::Wasm {
            wasi_version: WasiVersion::Preview2,
        };
        let wasm_clone = wasm.clone();
        assert_eq!(wasm, wasm_clone);
    }

    #[test]
    fn test_artifact_type_display_all_versions() {
        let p1 = ArtifactType::Wasm {
            wasi_version: WasiVersion::Preview1,
        };
        assert_eq!(p1.to_string(), "wasm (wasip1)");

        let p2 = ArtifactType::Wasm {
            wasi_version: WasiVersion::Preview2,
        };
        assert_eq!(p2.to_string(), "wasm (wasip2)");

        let unknown = ArtifactType::Wasm {
            wasi_version: WasiVersion::Unknown,
        };
        assert_eq!(unknown.to_string(), "wasm (unknown)");
    }
}
