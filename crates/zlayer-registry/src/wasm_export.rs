//! WASM export functionality for OCI registries
//!
//! This module provides functionality to export WebAssembly binaries as OCI artifacts,
//! conforming to the WASM OCI artifact specification.
//!
//! ## OCI WASM Artifact Structure
//!
//! WASM artifacts in OCI registries follow a specific format:
//! - Config blob: Empty JSON object with media type `application/vnd.wasm.config.v0+json`
//! - Layer: The WASM binary with media type `application/wasm`
//! - Artifact type: `application/vnd.wasm.component.v1+wasm` (WASIp2) or
//!   `application/vnd.wasm.module.v1+wasm` (WASIp1)
//!
//! ## Usage
//!
//! ```rust,no_run
//! use zlayer_registry::wasm_export::{WasmExportConfig, export_wasm_as_oci};
//! use zlayer_registry::wasm::WasiVersion;
//! use std::path::PathBuf;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let config = WasmExportConfig {
//!     wasm_path: PathBuf::from("/path/to/module.wasm"),
//!     module_name: "my-module".to_string(),
//!     wasi_version: Some(WasiVersion::Preview2),
//!     annotations: Default::default(),
//! };
//!
//! let result = export_wasm_as_oci(&config).await?;
//! println!("WASM manifest digest: {}", result.manifest_digest);
//! # Ok(())
//! # }
//! ```

use crate::cache::compute_digest;
use crate::wasm::{
    WasiVersion, WASM_COMPONENT_ARTIFACT_TYPE, WASM_CONFIG_MEDIA_TYPE_V0,
    WASM_LAYER_MEDIA_TYPE_GENERIC, WASM_MODULE_ARTIFACT_TYPE,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use thiserror::Error;
use tokio::fs;
use tracing::instrument;

/// WASM binary magic bytes: `\0asm`
const WASM_MAGIC_BYTES: [u8; 4] = [0x00, 0x61, 0x73, 0x6d];

/// WASM binary version for core module (wasip1)
const WASM_VERSION_CORE: [u8; 4] = [0x01, 0x00, 0x00, 0x00];

/// Component model magic/version indicator
/// Components start with the core WASM magic followed by a component layer indicator
const COMPONENT_LAYER_ID: u8 = 0x0d;

/// Configuration for exporting a WASM binary as an OCI artifact
#[derive(Debug, Clone)]
pub struct WasmExportConfig {
    /// Path to the WASM binary file
    pub wasm_path: PathBuf,

    /// Name for the WASM module (used in annotations)
    pub module_name: String,

    /// WASI version to use. If None, will be auto-detected from the binary
    pub wasi_version: Option<WasiVersion>,

    /// Additional annotations to include in the manifest
    pub annotations: HashMap<String, String>,
}

/// Result of exporting a WASM binary as OCI artifact
#[derive(Debug, Clone)]
pub struct WasmExportResult {
    /// Digest of the manifest (sha256:...)
    pub manifest_digest: String,

    /// Size of the manifest in bytes
    pub manifest_size: u64,

    /// Digest of the WASM layer (sha256:...)
    pub wasm_layer_digest: String,

    /// Size of the WASM binary in bytes
    pub wasm_size: u64,

    /// Digest of the config blob (sha256:...)
    pub config_digest: String,

    /// Size of the config blob in bytes
    pub config_size: u64,

    /// Detected or specified WASI version
    pub wasi_version: WasiVersion,

    /// The artifact type used in the manifest
    pub artifact_type: String,

    /// The serialized manifest JSON
    pub manifest_json: Vec<u8>,

    /// The config blob content
    pub config_blob: Vec<u8>,

    /// The WASM binary content
    pub wasm_binary: Vec<u8>,
}

/// Errors that can occur during WASM export operations
#[derive(Debug, Error)]
pub enum WasmExportError {
    /// The WASM binary is invalid or malformed
    #[error("invalid WASM binary: {reason}")]
    InvalidWasmBinary {
        /// Description of why the binary is invalid
        reason: String,
    },

    /// IO error reading the WASM file
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    /// JSON serialization error
    #[error("serialization error: failed to serialize manifest or config")]
    SerializationError,

    /// Storage backend error
    #[error("storage error: {reason}")]
    StorageError {
        /// Description of the storage error
        reason: String,
    },
}

/// OCI Image Manifest structure for WASM artifacts
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OciManifest {
    /// Schema version (must be 2)
    #[serde(rename = "schemaVersion")]
    schema_version: u32,

    /// Media type of this manifest
    #[serde(rename = "mediaType")]
    media_type: String,

    /// Artifact type indicating this is a WASM artifact
    #[serde(rename = "artifactType", skip_serializing_if = "Option::is_none")]
    artifact_type: Option<String>,

    /// Config descriptor
    config: OciDescriptor,

    /// Layer descriptors
    layers: Vec<OciDescriptor>,

    /// Optional annotations
    #[serde(skip_serializing_if = "Option::is_none")]
    annotations: Option<HashMap<String, String>>,
}

/// OCI Content Descriptor
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OciDescriptor {
    /// MIME type of the referenced content
    #[serde(rename = "mediaType")]
    media_type: String,

    /// Content digest (e.g., "sha256:...")
    digest: String,

    /// Size of the content in bytes
    size: u64,

    /// Optional annotations
    #[serde(skip_serializing_if = "Option::is_none")]
    annotations: Option<HashMap<String, String>>,
}

/// Validate WASM binary magic bytes
fn validate_wasm_magic(data: &[u8]) -> Result<(), WasmExportError> {
    if data.len() < 8 {
        return Err(WasmExportError::InvalidWasmBinary {
            reason: "file too small to be a valid WASM binary (less than 8 bytes)".to_string(),
        });
    }

    if data[0..4] != WASM_MAGIC_BYTES {
        return Err(WasmExportError::InvalidWasmBinary {
            reason: format!(
                "invalid magic bytes: expected {:?}, got {:?}",
                WASM_MAGIC_BYTES,
                &data[0..4]
            ),
        });
    }

    Ok(())
}

/// Detect WASI version from WASM binary structure
///
/// This function analyzes the binary structure to determine if it's a
/// WASIp1 (core module) or WASIp2 (component model) binary.
///
/// The detection is based on:
/// - Core modules (WASIp1): Start with `\0asm` followed by version `0x01 0x00 0x00 0x00`
/// - Components (WASIp2): Start with `\0asm` followed by version bytes, then contain
///   a component layer indicator (0x0d) in the section IDs
fn detect_wasi_version_from_binary(data: &[u8]) -> WasiVersion {
    if data.len() < 8 {
        return WasiVersion::Unknown;
    }

    // Check for component model indicator
    // Components have a specific structure where the first section after the header
    // contains a component layer indicator
    let version_bytes = &data[4..8];

    if version_bytes == WASM_VERSION_CORE {
        // This is a core module format, but we need to check if it's actually
        // a component wrapped in the new component model format
        //
        // Component model uses section ID 0x0d for custom sections
        // We look for the component model layer indicator in the first few sections
        if data.len() > 8 {
            // Scan the first section ID to see if it's a component layer
            let first_section_id = data[8];
            if first_section_id == COMPONENT_LAYER_ID {
                return WasiVersion::Preview2;
            }

            // Also check for the component model custom section pattern
            // which can appear later in the binary
            if contains_component_indicator(data) {
                return WasiVersion::Preview2;
            }
        }

        return WasiVersion::Preview1;
    }

    // Check for newer component model version format
    // Component model version 0x0d indicates WASIp2
    if version_bytes[0] == COMPONENT_LAYER_ID {
        return WasiVersion::Preview2;
    }

    WasiVersion::Unknown
}

/// Check if the binary contains component model indicators
///
/// This scans for patterns that indicate this is a component model binary
fn contains_component_indicator(data: &[u8]) -> bool {
    // Component model binaries often contain specific custom sections
    // Look for the "component-type" custom section name
    const COMPONENT_TYPE_MARKER: &[u8] = b"component-type";
    const WIT_COMPONENT_MARKER: &[u8] = b"wit-component";

    // Simple sliding window search for markers
    for window in data.windows(COMPONENT_TYPE_MARKER.len()) {
        if window == COMPONENT_TYPE_MARKER {
            return true;
        }
    }

    for window in data.windows(WIT_COMPONENT_MARKER.len()) {
        if window == WIT_COMPONENT_MARKER {
            return true;
        }
    }

    false
}

/// Get the artifact type media type for a WASI version
fn artifact_type_for_wasi_version(version: &WasiVersion) -> String {
    match version {
        WasiVersion::Preview1 => WASM_MODULE_ARTIFACT_TYPE.to_string(),
        WasiVersion::Preview2 => WASM_COMPONENT_ARTIFACT_TYPE.to_string(),
        WasiVersion::Unknown => WASM_MODULE_ARTIFACT_TYPE.to_string(),
    }
}

/// Create an empty config blob for WASM artifacts
///
/// WASM OCI artifacts use an empty JSON object as the config blob
fn create_wasm_config_blob() -> Vec<u8> {
    b"{}".to_vec()
}

/// Export a WASM binary as an OCI artifact
///
/// This function reads a WASM binary file, validates it, detects the WASI version,
/// and creates the necessary OCI manifest and descriptors for storing it as an
/// OCI artifact.
///
/// # Arguments
///
/// * `config` - Configuration specifying the WASM file path, module name, and options
///
/// # Returns
///
/// Returns a `WasmExportResult` containing all the generated blobs and manifest
/// information needed to push the WASM artifact to an OCI registry.
///
/// # Errors
///
/// Returns an error if:
/// - The WASM file cannot be read
/// - The file is not a valid WASM binary (invalid magic bytes)
/// - The manifest cannot be serialized
///
/// # Example
///
/// ```rust,no_run
/// use zlayer_registry::wasm_export::{WasmExportConfig, export_wasm_as_oci};
/// use std::path::PathBuf;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let config = WasmExportConfig {
///     wasm_path: PathBuf::from("./target/wasm32-wasip2/release/my_module.wasm"),
///     module_name: "my-module".to_string(),
///     wasi_version: None, // Auto-detect
///     annotations: Default::default(),
/// };
///
/// let result = export_wasm_as_oci(&config).await?;
///
/// // The result contains all blobs ready for pushing to a registry:
/// // - result.manifest_json: The OCI manifest
/// // - result.config_blob: The config blob (empty JSON)
/// // - result.wasm_binary: The WASM binary itself
/// # Ok(())
/// # }
/// ```
#[instrument(
    name = "export_wasm_as_oci",
    skip(config),
    fields(
        wasm_path = %config.wasm_path.display(),
        module_name = %config.module_name,
    )
)]
pub async fn export_wasm_as_oci(
    config: &WasmExportConfig,
) -> Result<WasmExportResult, WasmExportError> {
    // Read the WASM binary
    tracing::debug!("reading WASM binary from disk");
    let wasm_binary = fs::read(&config.wasm_path).await?;

    // Validate magic bytes
    tracing::debug!("validating WASM magic bytes");
    validate_wasm_magic(&wasm_binary)?;

    // Determine WASI version
    let wasi_version = config.wasi_version.clone().unwrap_or_else(|| {
        tracing::debug!("auto-detecting WASI version from binary structure");
        detect_wasi_version_from_binary(&wasm_binary)
    });

    tracing::info!(
        wasi_version = %wasi_version,
        wasm_size = wasm_binary.len(),
        "validated WASM binary"
    );

    // Compute WASM layer digest
    let wasm_layer_digest = compute_digest(&wasm_binary);
    let wasm_size = wasm_binary.len() as u64;

    // Create config blob (empty JSON for WASM artifacts)
    let config_blob = create_wasm_config_blob();
    let config_digest = compute_digest(&config_blob);
    let config_size = config_blob.len() as u64;

    // Determine artifact type based on WASI version
    let artifact_type = artifact_type_for_wasi_version(&wasi_version);

    // Build layer annotations
    let mut layer_annotations = HashMap::new();
    layer_annotations.insert(
        "org.opencontainers.image.title".to_string(),
        format!("{}.wasm", config.module_name),
    );

    // Build manifest annotations
    let mut manifest_annotations = config.annotations.clone();
    if !manifest_annotations.contains_key("org.opencontainers.image.title") {
        manifest_annotations.insert(
            "org.opencontainers.image.title".to_string(),
            config.module_name.clone(),
        );
    }

    // Create the OCI manifest
    let manifest = OciManifest {
        schema_version: 2,
        media_type: "application/vnd.oci.image.manifest.v1+json".to_string(),
        artifact_type: Some(artifact_type.clone()),
        config: OciDescriptor {
            media_type: WASM_CONFIG_MEDIA_TYPE_V0.to_string(),
            digest: config_digest.clone(),
            size: config_size,
            annotations: None,
        },
        layers: vec![OciDescriptor {
            media_type: WASM_LAYER_MEDIA_TYPE_GENERIC.to_string(),
            digest: wasm_layer_digest.clone(),
            size: wasm_size,
            annotations: Some(layer_annotations),
        }],
        annotations: if manifest_annotations.is_empty() {
            None
        } else {
            Some(manifest_annotations)
        },
    };

    // Serialize the manifest
    let manifest_json =
        serde_json::to_vec_pretty(&manifest).map_err(|_| WasmExportError::SerializationError)?;

    let manifest_digest = compute_digest(&manifest_json);
    let manifest_size = manifest_json.len() as u64;

    tracing::info!(
        manifest_digest = %manifest_digest,
        manifest_size = manifest_size,
        wasm_layer_digest = %wasm_layer_digest,
        config_digest = %config_digest,
        artifact_type = %artifact_type,
        "created OCI manifest for WASM artifact"
    );

    Ok(WasmExportResult {
        manifest_digest,
        manifest_size,
        wasm_layer_digest,
        wasm_size,
        config_digest,
        config_size,
        wasi_version,
        artifact_type,
        manifest_json,
        config_blob,
        wasm_binary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // =========================================================================
    // WASM Binary Helpers
    // =========================================================================

    /// Create a minimal valid WASIp1 core module
    fn create_minimal_wasm_module() -> Vec<u8> {
        // Minimal valid WASM module:
        // Magic: \0asm
        // Version: 1.0.0.0
        // Empty module (no sections)
        vec![
            0x00, 0x61, 0x73, 0x6d, // Magic: \0asm
            0x01, 0x00, 0x00, 0x00, // Version: 1
        ]
    }

    /// Create a minimal WASIp1 module with a type section
    fn create_wasm_module_with_type_section() -> Vec<u8> {
        vec![
            0x00, 0x61, 0x73, 0x6d, // Magic: \0asm
            0x01, 0x00, 0x00, 0x00, // Version: 1
            0x01, // Type section ID
            0x04, // Section size: 4 bytes
            0x01, // Number of types: 1
            0x60, // Function type indicator
            0x00, // Number of parameters: 0
            0x00, // Number of results: 0
        ]
    }

    /// Create a WASIp1 module with a custom section (name section)
    fn create_wasm_module_with_custom_section() -> Vec<u8> {
        vec![
            0x00, 0x61, 0x73, 0x6d, // Magic: \0asm
            0x01, 0x00, 0x00, 0x00, // Version: 1
            0x00, // Custom section ID (0x00)
            0x05, // Section size: 5 bytes
            0x04, // Name length: 4
            0x6e, 0x61, 0x6d, 0x65, // "name"
        ]
    }

    /// Create a minimal WASIp2 component using component model layer version
    fn create_minimal_wasm_component() -> Vec<u8> {
        // Component model binaries start with the same magic but different version
        let mut data = vec![
            0x00, 0x61, 0x73, 0x6d, // Magic: \0asm
            0x0d, 0x00, 0x01, 0x00, // Component layer version (0x0d = 13)
        ];
        // Add component-type marker for detection
        data.extend_from_slice(b"component-type");
        data
    }

    /// Create a WASIp2 component with wit-component marker
    fn create_wasm_component_with_wit_marker() -> Vec<u8> {
        let mut data = vec![
            0x00, 0x61, 0x73, 0x6d, // Magic: \0asm
            0x0d, 0x00, 0x01, 0x00, // Component layer version
        ];
        // Add wit-component marker for detection
        data.extend_from_slice(b"wit-component");
        data
    }

    /// Create invalid bytes that look nothing like WASM
    fn create_invalid_bytes() -> Vec<u8> {
        vec![0xde, 0xad, 0xbe, 0xef, 0xca, 0xfe, 0xba, 0xbe]
    }

    /// Create bytes that have correct magic but wrong version structure
    fn create_invalid_version_bytes() -> Vec<u8> {
        vec![
            0x00, 0x61, 0x73, 0x6d, // Magic: \0asm
            0xff, 0xff, 0x00, 0x00, // Unusual version
        ]
    }

    // =========================================================================
    // WasmExportConfig Tests
    // =========================================================================

    #[test]
    fn test_wasm_export_config_creation_all_fields() {
        let mut annotations = HashMap::new();
        annotations.insert("key1".to_string(), "value1".to_string());
        annotations.insert("key2".to_string(), "value2".to_string());

        let config = WasmExportConfig {
            wasm_path: PathBuf::from("/path/to/module.wasm"),
            module_name: "my-module".to_string(),
            wasi_version: Some(WasiVersion::Preview2),
            annotations: annotations.clone(),
        };

        assert_eq!(config.wasm_path, PathBuf::from("/path/to/module.wasm"));
        assert_eq!(config.module_name, "my-module");
        assert_eq!(config.wasi_version, Some(WasiVersion::Preview2));
        assert_eq!(config.annotations.len(), 2);
        assert_eq!(config.annotations.get("key1"), Some(&"value1".to_string()));
    }

    #[test]
    fn test_wasm_export_config_creation_minimal_fields() {
        let config = WasmExportConfig {
            wasm_path: PathBuf::from("./module.wasm"),
            module_name: "minimal".to_string(),
            wasi_version: None,
            annotations: HashMap::new(),
        };

        assert_eq!(config.wasm_path, PathBuf::from("./module.wasm"));
        assert_eq!(config.module_name, "minimal");
        assert_eq!(config.wasi_version, None);
        assert!(config.annotations.is_empty());
    }

    #[test]
    fn test_wasm_export_config_clone() {
        let mut annotations = HashMap::new();
        annotations.insert("key".to_string(), "value".to_string());

        let config = WasmExportConfig {
            wasm_path: PathBuf::from("/test/path.wasm"),
            module_name: "test".to_string(),
            wasi_version: Some(WasiVersion::Preview1),
            annotations,
        };

        let cloned = config.clone();

        assert_eq!(cloned.wasm_path, config.wasm_path);
        assert_eq!(cloned.module_name, config.module_name);
        assert_eq!(cloned.wasi_version, config.wasi_version);
        assert_eq!(cloned.annotations, config.annotations);
    }

    #[test]
    fn test_wasm_export_config_debug() {
        let config = WasmExportConfig {
            wasm_path: PathBuf::from("/test.wasm"),
            module_name: "test".to_string(),
            wasi_version: None,
            annotations: HashMap::new(),
        };

        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("WasmExportConfig"));
        assert!(debug_str.contains("wasm_path"));
        assert!(debug_str.contains("module_name"));
    }

    // =========================================================================
    // WASM Binary Validation Tests
    // =========================================================================

    #[test]
    fn test_validate_wasm_magic_valid_module() {
        let valid_wasm = create_minimal_wasm_module();
        assert!(validate_wasm_magic(&valid_wasm).is_ok());
    }

    #[test]
    fn test_validate_wasm_magic_valid_module_with_type_section() {
        let valid_wasm = create_wasm_module_with_type_section();
        assert!(validate_wasm_magic(&valid_wasm).is_ok());
    }

    #[test]
    fn test_validate_wasm_magic_valid_component() {
        let valid_component = create_minimal_wasm_component();
        assert!(validate_wasm_magic(&valid_component).is_ok());
    }

    #[test]
    fn test_validate_wasm_magic_invalid_bytes() {
        let invalid_data = create_invalid_bytes();
        let result = validate_wasm_magic(&invalid_data);
        assert!(result.is_err());
        match result {
            Err(WasmExportError::InvalidWasmBinary { reason }) => {
                assert!(reason.contains("invalid magic bytes"));
            }
            _ => panic!("expected InvalidWasmBinary error"),
        }
    }

    #[test]
    fn test_validate_wasm_magic_wrong_first_byte() {
        // First byte is 0x01 instead of 0x00
        let wrong_first = vec![0x01, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        let result = validate_wasm_magic(&wrong_first);
        assert!(result.is_err());
        match result {
            Err(WasmExportError::InvalidWasmBinary { reason }) => {
                assert!(reason.contains("invalid magic bytes"));
            }
            _ => panic!("expected InvalidWasmBinary error"),
        }
    }

    #[test]
    fn test_validate_wasm_magic_empty_bytes() {
        let empty: Vec<u8> = vec![];
        let result = validate_wasm_magic(&empty);
        assert!(result.is_err());
        match result {
            Err(WasmExportError::InvalidWasmBinary { reason }) => {
                assert!(reason.contains("too small"));
            }
            _ => panic!("expected InvalidWasmBinary error for empty input"),
        }
    }

    #[test]
    fn test_validate_wasm_magic_truncated_4_bytes() {
        // Only magic, no version
        let too_small = vec![0x00, 0x61, 0x73, 0x6d];
        let result = validate_wasm_magic(&too_small);
        assert!(result.is_err());
        match result {
            Err(WasmExportError::InvalidWasmBinary { reason }) => {
                assert!(reason.contains("too small"));
            }
            _ => panic!("expected InvalidWasmBinary error"),
        }
    }

    #[test]
    fn test_validate_wasm_magic_truncated_7_bytes() {
        // Missing one byte of version
        let too_small = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00];
        let result = validate_wasm_magic(&too_small);
        assert!(result.is_err());
        match result {
            Err(WasmExportError::InvalidWasmBinary { reason }) => {
                assert!(reason.contains("too small"));
            }
            _ => panic!("expected InvalidWasmBinary error"),
        }
    }

    #[test]
    fn test_validate_wasm_magic_exact_8_bytes_valid() {
        // Exactly magic + version, no sections
        let exact = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        let result = validate_wasm_magic(&exact);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_wasm_magic_elf_header() {
        // ELF binary magic bytes
        let elf = vec![0x7f, 0x45, 0x4c, 0x46, 0x02, 0x01, 0x01, 0x00];
        let result = validate_wasm_magic(&elf);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_wasm_magic_pe_header() {
        // Windows PE header (MZ)
        let pe = vec![0x4d, 0x5a, 0x90, 0x00, 0x03, 0x00, 0x00, 0x00];
        let result = validate_wasm_magic(&pe);
        assert!(result.is_err());
    }

    // =========================================================================
    // WASI Version Detection Tests
    // =========================================================================

    #[test]
    fn test_detect_wasi_version_wasip1_minimal_module() {
        let core_module = create_minimal_wasm_module();
        let version = detect_wasi_version_from_binary(&core_module);
        assert_eq!(version, WasiVersion::Preview1);
    }

    #[test]
    fn test_detect_wasi_version_wasip1_with_type_section() {
        let module = create_wasm_module_with_type_section();
        let version = detect_wasi_version_from_binary(&module);
        assert_eq!(version, WasiVersion::Preview1);
    }

    #[test]
    fn test_detect_wasi_version_wasip1_with_custom_section() {
        let module = create_wasm_module_with_custom_section();
        let version = detect_wasi_version_from_binary(&module);
        assert_eq!(version, WasiVersion::Preview1);
    }

    #[test]
    fn test_detect_wasi_version_wasip2_component_type_marker() {
        let component = create_minimal_wasm_component();
        let version = detect_wasi_version_from_binary(&component);
        assert_eq!(version, WasiVersion::Preview2);
    }

    #[test]
    fn test_detect_wasi_version_wasip2_wit_component_marker() {
        let component = create_wasm_component_with_wit_marker();
        let version = detect_wasi_version_from_binary(&component);
        assert_eq!(version, WasiVersion::Preview2);
    }

    #[test]
    fn test_detect_wasi_version_empty_returns_unknown() {
        let empty: Vec<u8> = vec![];
        let version = detect_wasi_version_from_binary(&empty);
        assert_eq!(version, WasiVersion::Unknown);
    }

    #[test]
    fn test_detect_wasi_version_too_short_returns_unknown() {
        // Less than 8 bytes
        let short = vec![0x00, 0x61, 0x73, 0x6d, 0x01];
        let version = detect_wasi_version_from_binary(&short);
        assert_eq!(version, WasiVersion::Unknown);
    }

    #[test]
    fn test_detect_wasi_version_invalid_magic_returns_unknown() {
        let invalid = create_invalid_bytes();
        let version = detect_wasi_version_from_binary(&invalid);
        assert_eq!(version, WasiVersion::Unknown);
    }

    #[test]
    fn test_detect_wasi_version_unusual_version_returns_unknown() {
        // Valid magic but unusual version that doesn't match known patterns
        let unusual = create_invalid_version_bytes();
        let version = detect_wasi_version_from_binary(&unusual);
        assert_eq!(version, WasiVersion::Unknown);
    }

    // =========================================================================
    // Config Blob Creation Tests
    // =========================================================================

    #[test]
    fn test_create_wasm_config_blob_returns_empty_json() {
        let config_blob = create_wasm_config_blob();
        assert_eq!(config_blob, b"{}");
    }

    #[test]
    fn test_create_wasm_config_blob_valid_json() {
        let config_blob = create_wasm_config_blob();
        let parsed: Result<serde_json::Value, _> = serde_json::from_slice(&config_blob);
        assert!(parsed.is_ok());
        let value = parsed.unwrap();
        assert!(value.is_object());
        assert!(value.as_object().unwrap().is_empty());
    }

    #[test]
    fn test_create_wasm_config_blob_consistent_digest() {
        // Multiple calls should produce identical blob
        let blob1 = create_wasm_config_blob();
        let blob2 = create_wasm_config_blob();
        assert_eq!(blob1, blob2);

        // Verify digest is consistent
        let digest1 = compute_digest(&blob1);
        let digest2 = compute_digest(&blob2);
        assert_eq!(digest1, digest2);
    }

    // =========================================================================
    // Layer Descriptor Tests
    // =========================================================================

    #[tokio::test]
    async fn test_layer_descriptor_wasip1_media_type() {
        let temp_dir = TempDir::new().unwrap();
        let wasm_path = temp_dir.path().join("module.wasm");
        let wasm_data = create_minimal_wasm_module();
        fs::write(&wasm_path, &wasm_data).await.unwrap();

        let config = WasmExportConfig {
            wasm_path,
            module_name: "test-module".to_string(),
            wasi_version: Some(WasiVersion::Preview1),
            annotations: HashMap::new(),
        };

        let result = export_wasm_as_oci(&config).await.unwrap();
        let manifest: OciManifest = serde_json::from_slice(&result.manifest_json).unwrap();

        assert_eq!(manifest.layers.len(), 1);
        assert_eq!(manifest.layers[0].media_type, WASM_LAYER_MEDIA_TYPE_GENERIC);
    }

    #[tokio::test]
    async fn test_layer_descriptor_wasip2_media_type() {
        let temp_dir = TempDir::new().unwrap();
        let wasm_path = temp_dir.path().join("component.wasm");
        let wasm_data = create_minimal_wasm_component();
        fs::write(&wasm_path, &wasm_data).await.unwrap();

        let config = WasmExportConfig {
            wasm_path,
            module_name: "test-component".to_string(),
            wasi_version: Some(WasiVersion::Preview2),
            annotations: HashMap::new(),
        };

        let result = export_wasm_as_oci(&config).await.unwrap();
        let manifest: OciManifest = serde_json::from_slice(&result.manifest_json).unwrap();

        // Layer media type is the same for both WASIp1 and WASIp2
        assert_eq!(manifest.layers[0].media_type, WASM_LAYER_MEDIA_TYPE_GENERIC);
    }

    #[tokio::test]
    async fn test_layer_descriptor_digest_calculation() {
        let temp_dir = TempDir::new().unwrap();
        let wasm_path = temp_dir.path().join("module.wasm");
        let wasm_data = create_wasm_module_with_type_section();
        fs::write(&wasm_path, &wasm_data).await.unwrap();

        let config = WasmExportConfig {
            wasm_path,
            module_name: "test-module".to_string(),
            wasi_version: None,
            annotations: HashMap::new(),
        };

        let result = export_wasm_as_oci(&config).await.unwrap();
        let manifest: OciManifest = serde_json::from_slice(&result.manifest_json).unwrap();

        // Verify the digest matches what we compute directly
        let expected_digest = compute_digest(&wasm_data);
        assert_eq!(manifest.layers[0].digest, expected_digest);
        assert_eq!(result.wasm_layer_digest, expected_digest);
    }

    #[tokio::test]
    async fn test_layer_descriptor_size_calculation() {
        let temp_dir = TempDir::new().unwrap();
        let wasm_path = temp_dir.path().join("module.wasm");
        let wasm_data = create_wasm_module_with_type_section();
        let expected_size = wasm_data.len() as u64;
        fs::write(&wasm_path, &wasm_data).await.unwrap();

        let config = WasmExportConfig {
            wasm_path,
            module_name: "test-module".to_string(),
            wasi_version: None,
            annotations: HashMap::new(),
        };

        let result = export_wasm_as_oci(&config).await.unwrap();
        let manifest: OciManifest = serde_json::from_slice(&result.manifest_json).unwrap();

        assert_eq!(manifest.layers[0].size, expected_size);
        assert_eq!(result.wasm_size, expected_size);
    }

    #[tokio::test]
    async fn test_layer_descriptor_has_title_annotation() {
        let temp_dir = TempDir::new().unwrap();
        let wasm_path = temp_dir.path().join("module.wasm");
        fs::write(&wasm_path, &create_minimal_wasm_module())
            .await
            .unwrap();

        let config = WasmExportConfig {
            wasm_path,
            module_name: "my-custom-module".to_string(),
            wasi_version: None,
            annotations: HashMap::new(),
        };

        let result = export_wasm_as_oci(&config).await.unwrap();
        let manifest: OciManifest = serde_json::from_slice(&result.manifest_json).unwrap();

        let layer_annotations = manifest.layers[0].annotations.as_ref().unwrap();
        assert_eq!(
            layer_annotations.get("org.opencontainers.image.title"),
            Some(&"my-custom-module.wasm".to_string())
        );
    }

    // =========================================================================
    // Manifest Building Tests
    // =========================================================================

    #[tokio::test]
    async fn test_manifest_artifact_type_wasip1() {
        let temp_dir = TempDir::new().unwrap();
        let wasm_path = temp_dir.path().join("module.wasm");
        fs::write(&wasm_path, &create_minimal_wasm_module())
            .await
            .unwrap();

        let config = WasmExportConfig {
            wasm_path,
            module_name: "test".to_string(),
            wasi_version: Some(WasiVersion::Preview1),
            annotations: HashMap::new(),
        };

        let result = export_wasm_as_oci(&config).await.unwrap();
        let manifest: OciManifest = serde_json::from_slice(&result.manifest_json).unwrap();

        assert_eq!(
            manifest.artifact_type,
            Some(WASM_MODULE_ARTIFACT_TYPE.to_string())
        );
        assert_eq!(result.artifact_type, WASM_MODULE_ARTIFACT_TYPE);
    }

    #[tokio::test]
    async fn test_manifest_artifact_type_wasip2() {
        let temp_dir = TempDir::new().unwrap();
        let wasm_path = temp_dir.path().join("component.wasm");
        fs::write(&wasm_path, &create_minimal_wasm_component())
            .await
            .unwrap();

        let config = WasmExportConfig {
            wasm_path,
            module_name: "test".to_string(),
            wasi_version: Some(WasiVersion::Preview2),
            annotations: HashMap::new(),
        };

        let result = export_wasm_as_oci(&config).await.unwrap();
        let manifest: OciManifest = serde_json::from_slice(&result.manifest_json).unwrap();

        assert_eq!(
            manifest.artifact_type,
            Some(WASM_COMPONENT_ARTIFACT_TYPE.to_string())
        );
        assert_eq!(result.artifact_type, WASM_COMPONENT_ARTIFACT_TYPE);
    }

    #[tokio::test]
    async fn test_manifest_config_media_type() {
        let temp_dir = TempDir::new().unwrap();
        let wasm_path = temp_dir.path().join("module.wasm");
        fs::write(&wasm_path, &create_minimal_wasm_module())
            .await
            .unwrap();

        let config = WasmExportConfig {
            wasm_path,
            module_name: "test".to_string(),
            wasi_version: None,
            annotations: HashMap::new(),
        };

        let result = export_wasm_as_oci(&config).await.unwrap();
        let manifest: OciManifest = serde_json::from_slice(&result.manifest_json).unwrap();

        assert_eq!(manifest.config.media_type, WASM_CONFIG_MEDIA_TYPE_V0);
    }

    #[tokio::test]
    async fn test_manifest_config_digest_and_size() {
        let temp_dir = TempDir::new().unwrap();
        let wasm_path = temp_dir.path().join("module.wasm");
        fs::write(&wasm_path, &create_minimal_wasm_module())
            .await
            .unwrap();

        let config = WasmExportConfig {
            wasm_path,
            module_name: "test".to_string(),
            wasi_version: None,
            annotations: HashMap::new(),
        };

        let result = export_wasm_as_oci(&config).await.unwrap();
        let manifest: OciManifest = serde_json::from_slice(&result.manifest_json).unwrap();

        let config_blob = create_wasm_config_blob();
        let expected_digest = compute_digest(&config_blob);
        let expected_size = config_blob.len() as u64;

        assert_eq!(manifest.config.digest, expected_digest);
        assert_eq!(manifest.config.size, expected_size);
        assert_eq!(result.config_digest, expected_digest);
        assert_eq!(result.config_size, expected_size);
    }

    #[tokio::test]
    async fn test_manifest_schema_version() {
        let temp_dir = TempDir::new().unwrap();
        let wasm_path = temp_dir.path().join("module.wasm");
        fs::write(&wasm_path, &create_minimal_wasm_module())
            .await
            .unwrap();

        let config = WasmExportConfig {
            wasm_path,
            module_name: "test".to_string(),
            wasi_version: None,
            annotations: HashMap::new(),
        };

        let result = export_wasm_as_oci(&config).await.unwrap();
        let manifest: OciManifest = serde_json::from_slice(&result.manifest_json).unwrap();

        assert_eq!(manifest.schema_version, 2);
    }

    #[tokio::test]
    async fn test_manifest_media_type() {
        let temp_dir = TempDir::new().unwrap();
        let wasm_path = temp_dir.path().join("module.wasm");
        fs::write(&wasm_path, &create_minimal_wasm_module())
            .await
            .unwrap();

        let config = WasmExportConfig {
            wasm_path,
            module_name: "test".to_string(),
            wasi_version: None,
            annotations: HashMap::new(),
        };

        let result = export_wasm_as_oci(&config).await.unwrap();
        let manifest: OciManifest = serde_json::from_slice(&result.manifest_json).unwrap();

        assert_eq!(
            manifest.media_type,
            "application/vnd.oci.image.manifest.v1+json"
        );
    }

    #[tokio::test]
    async fn test_manifest_layers_count() {
        let temp_dir = TempDir::new().unwrap();
        let wasm_path = temp_dir.path().join("module.wasm");
        fs::write(&wasm_path, &create_minimal_wasm_module())
            .await
            .unwrap();

        let config = WasmExportConfig {
            wasm_path,
            module_name: "test".to_string(),
            wasi_version: None,
            annotations: HashMap::new(),
        };

        let result = export_wasm_as_oci(&config).await.unwrap();
        let manifest: OciManifest = serde_json::from_slice(&result.manifest_json).unwrap();

        // WASM artifacts have exactly one layer
        assert_eq!(manifest.layers.len(), 1);
    }

    #[tokio::test]
    async fn test_manifest_annotations_propagate() {
        let temp_dir = TempDir::new().unwrap();
        let wasm_path = temp_dir.path().join("module.wasm");
        fs::write(&wasm_path, &create_minimal_wasm_module())
            .await
            .unwrap();

        let mut annotations = HashMap::new();
        annotations.insert("org.example.custom".to_string(), "custom-value".to_string());
        annotations.insert("org.example.version".to_string(), "2.0.0".to_string());

        let config = WasmExportConfig {
            wasm_path,
            module_name: "annotated-test".to_string(),
            wasi_version: None,
            annotations,
        };

        let result = export_wasm_as_oci(&config).await.unwrap();
        let manifest: OciManifest = serde_json::from_slice(&result.manifest_json).unwrap();

        let manifest_annotations = manifest.annotations.unwrap();
        assert_eq!(
            manifest_annotations.get("org.example.custom"),
            Some(&"custom-value".to_string())
        );
        assert_eq!(
            manifest_annotations.get("org.example.version"),
            Some(&"2.0.0".to_string())
        );
    }

    #[tokio::test]
    async fn test_manifest_auto_adds_title_annotation() {
        let temp_dir = TempDir::new().unwrap();
        let wasm_path = temp_dir.path().join("module.wasm");
        fs::write(&wasm_path, &create_minimal_wasm_module())
            .await
            .unwrap();

        let config = WasmExportConfig {
            wasm_path,
            module_name: "auto-title-module".to_string(),
            wasi_version: None,
            annotations: HashMap::new(), // Empty - should auto-add title
        };

        let result = export_wasm_as_oci(&config).await.unwrap();
        let manifest: OciManifest = serde_json::from_slice(&result.manifest_json).unwrap();

        let manifest_annotations = manifest.annotations.unwrap();
        assert_eq!(
            manifest_annotations.get("org.opencontainers.image.title"),
            Some(&"auto-title-module".to_string())
        );
    }

    #[tokio::test]
    async fn test_manifest_does_not_override_existing_title() {
        let temp_dir = TempDir::new().unwrap();
        let wasm_path = temp_dir.path().join("module.wasm");
        fs::write(&wasm_path, &create_minimal_wasm_module())
            .await
            .unwrap();

        let mut annotations = HashMap::new();
        annotations.insert(
            "org.opencontainers.image.title".to_string(),
            "custom-title".to_string(),
        );

        let config = WasmExportConfig {
            wasm_path,
            module_name: "module-name".to_string(),
            wasi_version: None,
            annotations,
        };

        let result = export_wasm_as_oci(&config).await.unwrap();
        let manifest: OciManifest = serde_json::from_slice(&result.manifest_json).unwrap();

        let manifest_annotations = manifest.annotations.unwrap();
        // Should keep the custom title, not replace with module_name
        assert_eq!(
            manifest_annotations.get("org.opencontainers.image.title"),
            Some(&"custom-title".to_string())
        );
    }

    #[tokio::test]
    async fn test_manifest_no_annotations_when_empty() {
        let temp_dir = TempDir::new().unwrap();
        let wasm_path = temp_dir.path().join("module.wasm");
        fs::write(&wasm_path, &create_minimal_wasm_module())
            .await
            .unwrap();

        // Even with empty annotations, title is auto-added
        let config = WasmExportConfig {
            wasm_path,
            module_name: "test".to_string(),
            wasi_version: None,
            annotations: HashMap::new(),
        };

        let result = export_wasm_as_oci(&config).await.unwrap();
        let manifest: OciManifest = serde_json::from_slice(&result.manifest_json).unwrap();

        // Annotations should be present because title is auto-added
        assert!(manifest.annotations.is_some());
    }

    // =========================================================================
    // Export Integration Tests
    // =========================================================================

    #[tokio::test]
    async fn test_export_wasip1_full_flow() {
        let temp_dir = TempDir::new().unwrap();
        let wasm_path = temp_dir.path().join("wasip1_module.wasm");
        let wasm_data = create_wasm_module_with_type_section();
        fs::write(&wasm_path, &wasm_data).await.unwrap();

        let mut annotations = HashMap::new();
        annotations.insert("org.example.env".to_string(), "production".to_string());

        let config = WasmExportConfig {
            wasm_path: wasm_path.clone(),
            module_name: "wasip1-integration-test".to_string(),
            wasi_version: None, // Auto-detect
            annotations,
        };

        let result = export_wasm_as_oci(&config).await.unwrap();

        // Verify overall result structure
        assert!(result.manifest_digest.starts_with("sha256:"));
        assert!(result.wasm_layer_digest.starts_with("sha256:"));
        assert!(result.config_digest.starts_with("sha256:"));

        // Verify WASI version detection
        assert_eq!(result.wasi_version, WasiVersion::Preview1);
        assert_eq!(result.artifact_type, WASM_MODULE_ARTIFACT_TYPE);

        // Verify sizes
        assert_eq!(result.wasm_size, wasm_data.len() as u64);
        assert_eq!(result.config_size, 2); // "{}"
        assert!(result.manifest_size > 0);

        // Verify binary content
        assert_eq!(result.wasm_binary, wasm_data);
        assert_eq!(result.config_blob, b"{}");

        // Verify manifest JSON is well-formed
        let manifest: OciManifest = serde_json::from_slice(&result.manifest_json).unwrap();
        assert_eq!(manifest.schema_version, 2);
        assert_eq!(
            manifest.artifact_type,
            Some(WASM_MODULE_ARTIFACT_TYPE.to_string())
        );
        assert_eq!(manifest.layers.len(), 1);
        assert_eq!(manifest.layers[0].digest, result.wasm_layer_digest);
        assert_eq!(manifest.config.digest, result.config_digest);

        // Verify custom annotations
        let annotations = manifest.annotations.unwrap();
        assert_eq!(
            annotations.get("org.example.env"),
            Some(&"production".to_string())
        );
    }

    #[tokio::test]
    async fn test_export_wasip2_full_flow() {
        let temp_dir = TempDir::new().unwrap();
        let wasm_path = temp_dir.path().join("wasip2_component.wasm");
        let wasm_data = create_minimal_wasm_component();
        fs::write(&wasm_path, &wasm_data).await.unwrap();

        let config = WasmExportConfig {
            wasm_path: wasm_path.clone(),
            module_name: "wasip2-integration-test".to_string(),
            wasi_version: None, // Auto-detect
            annotations: HashMap::new(),
        };

        let result = export_wasm_as_oci(&config).await.unwrap();

        // Verify WASI version detection
        assert_eq!(result.wasi_version, WasiVersion::Preview2);
        assert_eq!(result.artifact_type, WASM_COMPONENT_ARTIFACT_TYPE);

        // Verify manifest structure
        let manifest: OciManifest = serde_json::from_slice(&result.manifest_json).unwrap();
        assert_eq!(
            manifest.artifact_type,
            Some(WASM_COMPONENT_ARTIFACT_TYPE.to_string())
        );
    }

    #[tokio::test]
    async fn test_export_with_explicit_version_override() {
        let temp_dir = TempDir::new().unwrap();
        let wasm_path = temp_dir.path().join("module.wasm");
        // Write a WASIp1 module
        let wasm_data = create_minimal_wasm_module();
        fs::write(&wasm_path, &wasm_data).await.unwrap();

        // But specify WASIp2 explicitly
        let config = WasmExportConfig {
            wasm_path,
            module_name: "override-test".to_string(),
            wasi_version: Some(WasiVersion::Preview2),
            annotations: HashMap::new(),
        };

        let result = export_wasm_as_oci(&config).await.unwrap();

        // Should use the explicit version, not auto-detected
        assert_eq!(result.wasi_version, WasiVersion::Preview2);
        assert_eq!(result.artifact_type, WASM_COMPONENT_ARTIFACT_TYPE);
    }

    #[tokio::test]
    async fn test_export_error_invalid_binary() {
        let temp_dir = TempDir::new().unwrap();
        let wasm_path = temp_dir.path().join("invalid.wasm");
        fs::write(&wasm_path, b"not a wasm file at all")
            .await
            .unwrap();

        let config = WasmExportConfig {
            wasm_path,
            module_name: "invalid".to_string(),
            wasi_version: None,
            annotations: HashMap::new(),
        };

        let result = export_wasm_as_oci(&config).await;
        assert!(result.is_err());

        match result {
            Err(WasmExportError::InvalidWasmBinary { reason }) => {
                assert!(reason.contains("invalid magic bytes"));
            }
            other => panic!("expected InvalidWasmBinary error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_export_error_truncated_binary() {
        let temp_dir = TempDir::new().unwrap();
        let wasm_path = temp_dir.path().join("truncated.wasm");
        // Only magic, missing version bytes
        fs::write(&wasm_path, &[0x00, 0x61, 0x73, 0x6d])
            .await
            .unwrap();

        let config = WasmExportConfig {
            wasm_path,
            module_name: "truncated".to_string(),
            wasi_version: None,
            annotations: HashMap::new(),
        };

        let result = export_wasm_as_oci(&config).await;
        assert!(result.is_err());

        match result {
            Err(WasmExportError::InvalidWasmBinary { reason }) => {
                assert!(reason.contains("too small"));
            }
            other => panic!("expected InvalidWasmBinary error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_export_error_missing_file() {
        let config = WasmExportConfig {
            wasm_path: PathBuf::from("/nonexistent/path/to/module.wasm"),
            module_name: "missing".to_string(),
            wasi_version: None,
            annotations: HashMap::new(),
        };

        let result = export_wasm_as_oci(&config).await;
        assert!(result.is_err());

        match result {
            Err(WasmExportError::IoError(e)) => {
                assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
            }
            other => panic!("expected IoError, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_export_error_permission_denied() {
        // Skip this test on non-Unix systems where permission behavior differs
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let temp_dir = TempDir::new().unwrap();
            let wasm_path = temp_dir.path().join("unreadable.wasm");
            fs::write(&wasm_path, &create_minimal_wasm_module())
                .await
                .unwrap();

            // Remove read permissions
            let metadata = std::fs::metadata(&wasm_path).unwrap();
            let mut perms = metadata.permissions();
            perms.set_mode(0o000);
            std::fs::set_permissions(&wasm_path, perms).unwrap();

            let config = WasmExportConfig {
                wasm_path: wasm_path.clone(),
                module_name: "unreadable".to_string(),
                wasi_version: None,
                annotations: HashMap::new(),
            };

            let result = export_wasm_as_oci(&config).await;
            assert!(result.is_err());

            match &result {
                Err(WasmExportError::IoError(e)) => {
                    assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
                }
                other => panic!("expected IoError(PermissionDenied), got {:?}", other),
            }

            // Restore permissions for cleanup
            let mut perms = std::fs::metadata(&wasm_path).unwrap().permissions();
            perms.set_mode(0o644);
            std::fs::set_permissions(&wasm_path, perms).unwrap();
        }
    }

    // =========================================================================
    // Artifact Type Mapping Tests
    // =========================================================================

    #[test]
    fn test_artifact_type_for_wasi_version_preview1() {
        assert_eq!(
            artifact_type_for_wasi_version(&WasiVersion::Preview1),
            WASM_MODULE_ARTIFACT_TYPE
        );
    }

    #[test]
    fn test_artifact_type_for_wasi_version_preview2() {
        assert_eq!(
            artifact_type_for_wasi_version(&WasiVersion::Preview2),
            WASM_COMPONENT_ARTIFACT_TYPE
        );
    }

    #[test]
    fn test_artifact_type_for_wasi_version_unknown() {
        // Unknown defaults to module type
        assert_eq!(
            artifact_type_for_wasi_version(&WasiVersion::Unknown),
            WASM_MODULE_ARTIFACT_TYPE
        );
    }

    // =========================================================================
    // Component Indicator Detection Tests
    // =========================================================================

    #[test]
    fn test_contains_component_indicator_component_type() {
        let mut data = vec![0x00; 100];
        data[50..64].copy_from_slice(b"component-type");
        assert!(contains_component_indicator(&data));
    }

    #[test]
    fn test_contains_component_indicator_wit_component() {
        let mut data = vec![0x00; 100];
        data[40..53].copy_from_slice(b"wit-component");
        assert!(contains_component_indicator(&data));
    }

    #[test]
    fn test_contains_component_indicator_at_start() {
        let mut data = vec![0x00; 100];
        data[0..14].copy_from_slice(b"component-type");
        assert!(contains_component_indicator(&data));
    }

    #[test]
    fn test_contains_component_indicator_at_end() {
        let mut data = vec![0x00; 100];
        data[86..100].copy_from_slice(b"component-type");
        assert!(contains_component_indicator(&data));
    }

    #[test]
    fn test_contains_component_indicator_none() {
        let data = vec![0x00; 100];
        assert!(!contains_component_indicator(&data));
    }

    #[test]
    fn test_contains_component_indicator_partial_match() {
        let mut data = vec![0x00; 100];
        // Only partial marker - should not match
        data[50..60].copy_from_slice(b"component-");
        assert!(!contains_component_indicator(&data));
    }

    #[test]
    fn test_contains_component_indicator_empty() {
        let data: Vec<u8> = vec![];
        assert!(!contains_component_indicator(&data));
    }

    #[test]
    fn test_contains_component_indicator_too_short() {
        let data = vec![0x00; 5]; // Shorter than marker
        assert!(!contains_component_indicator(&data));
    }

    // =========================================================================
    // Error Display Tests
    // =========================================================================

    #[test]
    fn test_wasm_export_error_display_invalid_binary() {
        let err = WasmExportError::InvalidWasmBinary {
            reason: "test reason".to_string(),
        };
        assert_eq!(err.to_string(), "invalid WASM binary: test reason");
    }

    #[test]
    fn test_wasm_export_error_display_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err = WasmExportError::IoError(io_err);
        assert!(err.to_string().contains("IO error"));
    }

    #[test]
    fn test_wasm_export_error_display_serialization() {
        let err = WasmExportError::SerializationError;
        assert_eq!(
            err.to_string(),
            "serialization error: failed to serialize manifest or config"
        );
    }

    #[test]
    fn test_wasm_export_error_display_storage() {
        let err = WasmExportError::StorageError {
            reason: "storage failed".to_string(),
        };
        assert_eq!(err.to_string(), "storage error: storage failed");
    }

    #[test]
    fn test_wasm_export_error_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
        let export_err: WasmExportError = io_err.into();
        match export_err {
            WasmExportError::IoError(e) => {
                assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
            }
            _ => panic!("expected IoError variant"),
        }
    }

    // =========================================================================
    // Result Structure Tests
    // =========================================================================

    #[tokio::test]
    async fn test_wasm_export_result_all_fields_populated() {
        let temp_dir = TempDir::new().unwrap();
        let wasm_path = temp_dir.path().join("module.wasm");
        let wasm_data = create_minimal_wasm_module();
        fs::write(&wasm_path, &wasm_data).await.unwrap();

        let config = WasmExportConfig {
            wasm_path,
            module_name: "test".to_string(),
            wasi_version: None,
            annotations: HashMap::new(),
        };

        let result = export_wasm_as_oci(&config).await.unwrap();

        // All digest fields should be populated and valid
        assert!(result.manifest_digest.starts_with("sha256:"));
        assert_eq!(result.manifest_digest.len(), 7 + 64); // "sha256:" + 64 hex chars

        assert!(result.wasm_layer_digest.starts_with("sha256:"));
        assert_eq!(result.wasm_layer_digest.len(), 7 + 64);

        assert!(result.config_digest.starts_with("sha256:"));
        assert_eq!(result.config_digest.len(), 7 + 64);

        // Size fields should be non-zero (except possibly for very small inputs)
        assert!(result.manifest_size > 0);
        assert!(result.wasm_size > 0);
        assert!(result.config_size > 0);

        // Binary content should be populated
        assert!(!result.manifest_json.is_empty());
        assert!(!result.wasm_binary.is_empty());
        assert!(!result.config_blob.is_empty());

        // Version and artifact type should be set
        assert!(result.artifact_type.contains("wasm"));
    }

    #[test]
    fn test_wasm_export_result_clone() {
        let result = WasmExportResult {
            manifest_digest: "sha256:abc123".to_string(),
            manifest_size: 100,
            wasm_layer_digest: "sha256:def456".to_string(),
            wasm_size: 200,
            config_digest: "sha256:ghi789".to_string(),
            config_size: 2,
            wasi_version: WasiVersion::Preview1,
            artifact_type: WASM_MODULE_ARTIFACT_TYPE.to_string(),
            manifest_json: b"{}".to_vec(),
            config_blob: b"{}".to_vec(),
            wasm_binary: vec![0x00, 0x61, 0x73, 0x6d],
        };

        let cloned = result.clone();

        assert_eq!(cloned.manifest_digest, result.manifest_digest);
        assert_eq!(cloned.wasm_layer_digest, result.wasm_layer_digest);
        assert_eq!(cloned.config_digest, result.config_digest);
        assert_eq!(cloned.manifest_size, result.manifest_size);
        assert_eq!(cloned.wasm_size, result.wasm_size);
        assert_eq!(cloned.config_size, result.config_size);
        assert_eq!(cloned.wasi_version, result.wasi_version);
        assert_eq!(cloned.artifact_type, result.artifact_type);
        assert_eq!(cloned.manifest_json, result.manifest_json);
        assert_eq!(cloned.config_blob, result.config_blob);
        assert_eq!(cloned.wasm_binary, result.wasm_binary);
    }

    #[test]
    fn test_wasm_export_result_debug() {
        let result = WasmExportResult {
            manifest_digest: "sha256:test".to_string(),
            manifest_size: 100,
            wasm_layer_digest: "sha256:wasm".to_string(),
            wasm_size: 200,
            config_digest: "sha256:config".to_string(),
            config_size: 2,
            wasi_version: WasiVersion::Preview2,
            artifact_type: WASM_COMPONENT_ARTIFACT_TYPE.to_string(),
            manifest_json: vec![],
            config_blob: vec![],
            wasm_binary: vec![],
        };

        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains("WasmExportResult"));
        assert!(debug_str.contains("manifest_digest"));
        assert!(debug_str.contains("Preview2"));
    }

    // =========================================================================
    // OCI Manifest Structure Tests
    // =========================================================================

    #[test]
    fn test_oci_manifest_serialization() {
        let manifest = OciManifest {
            schema_version: 2,
            media_type: "application/vnd.oci.image.manifest.v1+json".to_string(),
            artifact_type: Some(WASM_MODULE_ARTIFACT_TYPE.to_string()),
            config: OciDescriptor {
                media_type: WASM_CONFIG_MEDIA_TYPE_V0.to_string(),
                digest: "sha256:test123".to_string(),
                size: 2,
                annotations: None,
            },
            layers: vec![OciDescriptor {
                media_type: WASM_LAYER_MEDIA_TYPE_GENERIC.to_string(),
                digest: "sha256:layer456".to_string(),
                size: 100,
                annotations: Some({
                    let mut map = HashMap::new();
                    map.insert(
                        "org.opencontainers.image.title".to_string(),
                        "test.wasm".to_string(),
                    );
                    map
                }),
            }],
            annotations: None,
        };

        let json = serde_json::to_string(&manifest).unwrap();
        assert!(json.contains("schemaVersion"));
        assert!(json.contains("mediaType"));
        assert!(json.contains("artifactType"));
        assert!(json.contains("config"));
        assert!(json.contains("layers"));
    }

    #[test]
    fn test_oci_manifest_deserialization() {
        let json = r#"{
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "artifactType": "application/vnd.wasm.module.v1+wasm",
            "config": {
                "mediaType": "application/vnd.wasm.config.v0+json",
                "digest": "sha256:abc",
                "size": 2
            },
            "layers": [{
                "mediaType": "application/wasm",
                "digest": "sha256:def",
                "size": 100
            }]
        }"#;

        let manifest: OciManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.schema_version, 2);
        assert_eq!(
            manifest.artifact_type,
            Some(WASM_MODULE_ARTIFACT_TYPE.to_string())
        );
        assert_eq!(manifest.layers.len(), 1);
    }

    #[test]
    fn test_oci_descriptor_serialization() {
        let descriptor = OciDescriptor {
            media_type: "application/wasm".to_string(),
            digest: "sha256:test".to_string(),
            size: 12345,
            annotations: Some({
                let mut map = HashMap::new();
                map.insert("key".to_string(), "value".to_string());
                map
            }),
        };

        let json = serde_json::to_string(&descriptor).unwrap();
        assert!(json.contains("mediaType"));
        assert!(json.contains("digest"));
        assert!(json.contains("size"));
        assert!(json.contains("annotations"));
    }

    #[test]
    fn test_oci_manifest_skip_empty_annotations() {
        let manifest = OciManifest {
            schema_version: 2,
            media_type: "application/vnd.oci.image.manifest.v1+json".to_string(),
            artifact_type: None,
            config: OciDescriptor {
                media_type: WASM_CONFIG_MEDIA_TYPE_V0.to_string(),
                digest: "sha256:test".to_string(),
                size: 2,
                annotations: None,
            },
            layers: vec![],
            annotations: None, // Should be skipped in serialization
        };

        let json = serde_json::to_string(&manifest).unwrap();
        // annotations field should not appear when None
        assert!(!json.contains("annotations"));
    }

    #[test]
    fn test_oci_manifest_skip_empty_artifact_type() {
        let manifest = OciManifest {
            schema_version: 2,
            media_type: "application/vnd.oci.image.manifest.v1+json".to_string(),
            artifact_type: None, // Should be skipped
            config: OciDescriptor {
                media_type: WASM_CONFIG_MEDIA_TYPE_V0.to_string(),
                digest: "sha256:test".to_string(),
                size: 2,
                annotations: None,
            },
            layers: vec![],
            annotations: None,
        };

        let json = serde_json::to_string(&manifest).unwrap();
        // artifactType should not appear when None
        assert!(!json.contains("artifactType"));
    }

    // =========================================================================
    // Digest Consistency Tests
    // =========================================================================

    #[tokio::test]
    async fn test_digest_consistency_multiple_exports() {
        let temp_dir = TempDir::new().unwrap();
        let wasm_path = temp_dir.path().join("module.wasm");
        let wasm_data = create_minimal_wasm_module();
        fs::write(&wasm_path, &wasm_data).await.unwrap();

        let config = WasmExportConfig {
            wasm_path: wasm_path.clone(),
            module_name: "consistency-test".to_string(),
            wasi_version: None,
            annotations: HashMap::new(),
        };

        let result1 = export_wasm_as_oci(&config).await.unwrap();
        let result2 = export_wasm_as_oci(&config).await.unwrap();

        // Same input should produce same digests
        assert_eq!(result1.manifest_digest, result2.manifest_digest);
        assert_eq!(result1.wasm_layer_digest, result2.wasm_layer_digest);
        assert_eq!(result1.config_digest, result2.config_digest);
    }

    #[tokio::test]
    async fn test_digest_changes_with_different_binary() {
        let temp_dir = TempDir::new().unwrap();

        // First export
        let wasm_path1 = temp_dir.path().join("module1.wasm");
        fs::write(&wasm_path1, &create_minimal_wasm_module())
            .await
            .unwrap();

        let config1 = WasmExportConfig {
            wasm_path: wasm_path1,
            module_name: "test".to_string(),
            wasi_version: None,
            annotations: HashMap::new(),
        };

        let result1 = export_wasm_as_oci(&config1).await.unwrap();

        // Second export with different binary
        let wasm_path2 = temp_dir.path().join("module2.wasm");
        fs::write(&wasm_path2, &create_wasm_module_with_type_section())
            .await
            .unwrap();

        let config2 = WasmExportConfig {
            wasm_path: wasm_path2,
            module_name: "test".to_string(),
            wasi_version: None,
            annotations: HashMap::new(),
        };

        let result2 = export_wasm_as_oci(&config2).await.unwrap();

        // Different binaries should produce different layer digests
        assert_ne!(result1.wasm_layer_digest, result2.wasm_layer_digest);
        // And different manifest digests (since layer digest changed)
        assert_ne!(result1.manifest_digest, result2.manifest_digest);
        // But config digest should be the same (empty JSON)
        assert_eq!(result1.config_digest, result2.config_digest);
    }
}
