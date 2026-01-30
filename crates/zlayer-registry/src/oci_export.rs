//! OCI Image Layout export/import functionality
//!
//! This module provides functionality to export images to OCI Image Layout format
//! (as tar archives) and import images from OCI Image Layout archives.
//!
//! ## OCI Image Layout Specification
//!
//! The OCI Image Layout defines a directory structure for storing OCI images:
//!
//! ```text
//! image-layout/
//! |-- blobs/
//! |   |-- sha256/
//! |       |-- <digest>   # blob content (layers, config, manifest)
//! |-- oci-layout         # JSON file marking OCI layout version
//! |-- index.json         # image index referencing manifests
//! ```
//!
//! ## Usage
//!
//! ```rust,no_run
//! use zlayer_registry::oci_export::{export_image, import_image};
//! use zlayer_registry::LocalRegistry;
//! use std::path::Path;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let registry = LocalRegistry::new("/var/lib/zlayer/registry".into()).await?;
//!
//! // Export an image to a tar.gz file
//! let info = export_image(&registry, "myapp:latest", Path::new("/tmp/myapp.tar.gz")).await?;
//! println!("Exported {} layers, digest: {}", info.layers, info.digest);
//!
//! // Import an image from a tar file
//! let info = import_image(&registry, Path::new("/tmp/myapp.tar.gz"), Some("myapp:imported")).await?;
//! println!("Imported {} layers, digest: {}", info.layers, info.digest);
//! # Ok(())
//! # }
//! ```

use crate::cache::compute_digest;
use crate::local_registry::LocalRegistry;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tar::{Archive, Builder};
use thiserror::Error;
use tokio::fs;

/// OCI Image Layout version marker file content
///
/// This struct represents the contents of the `oci-layout` file that
/// marks a directory as containing an OCI Image Layout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OciLayout {
    /// The version of the OCI Image Layout specification
    #[serde(rename = "imageLayoutVersion")]
    pub image_layout_version: String,
}

impl Default for OciLayout {
    fn default() -> Self {
        Self {
            image_layout_version: "1.0.0".to_string(),
        }
    }
}

/// OCI Image Index (index.json)
///
/// The image index is the entry point for an OCI Image Layout. It contains
/// references to one or more image manifests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OciIndex {
    /// Schema version (must be 2 for OCI Image Spec)
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,

    /// Media type of this index
    #[serde(rename = "mediaType", skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,

    /// List of manifest descriptors
    pub manifests: Vec<OciDescriptor>,

    /// Optional annotations
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<HashMap<String, String>>,
}

impl OciIndex {
    /// Create a new OCI Index with a single manifest
    pub fn new(manifest_descriptor: OciDescriptor) -> Self {
        Self {
            schema_version: 2,
            media_type: Some("application/vnd.oci.image.index.v1+json".to_string()),
            manifests: vec![manifest_descriptor],
            annotations: None,
        }
    }
}

/// OCI Content Descriptor
///
/// A descriptor is used to reference content by digest, media type, and size.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OciDescriptor {
    /// MIME type of the referenced content
    #[serde(rename = "mediaType")]
    pub media_type: String,

    /// Content digest (e.g., "sha256:...")
    pub digest: String,

    /// Size of the content in bytes
    pub size: u64,

    /// URLs from which this content may be downloaded
    #[serde(skip_serializing_if = "Option::is_none")]
    pub urls: Option<Vec<String>>,

    /// Optional annotations
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<HashMap<String, String>>,

    /// Platform specification (for manifest lists)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<OciPlatform>,
}

/// OCI Platform specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OciPlatform {
    /// Target architecture (e.g., "amd64", "arm64")
    pub architecture: String,

    /// Target operating system (e.g., "linux", "windows")
    pub os: String,

    /// Optional OS version
    #[serde(rename = "os.version", skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,

    /// Optional OS features
    #[serde(rename = "os.features", skip_serializing_if = "Option::is_none")]
    pub os_features: Option<Vec<String>>,

    /// Optional CPU variant
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
}

/// OCI Image Manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OciManifest {
    /// Schema version (must be 2)
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,

    /// Media type of this manifest
    #[serde(rename = "mediaType", skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,

    /// Config descriptor
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<OciDescriptor>,

    /// Layer descriptors
    #[serde(default)]
    pub layers: Vec<OciDescriptor>,

    /// Optional annotations
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<HashMap<String, String>>,
}

/// Information about an exported image
#[derive(Debug, Clone)]
pub struct ExportInfo {
    /// Manifest digest of the exported image
    pub digest: String,

    /// Total size of the exported archive in bytes
    pub size: u64,

    /// Number of layers in the image
    pub layers: usize,

    /// Path to the output file
    pub output_path: PathBuf,
}

/// Information about an imported image
#[derive(Debug, Clone)]
pub struct ImportInfo {
    /// Manifest digest of the imported image
    pub digest: String,

    /// Tag applied to the imported image (if any)
    pub tag: Option<String>,

    /// Number of layers in the image
    pub layers: usize,
}

/// Errors that can occur during export operations
#[derive(Debug, Error)]
pub enum ExportError {
    /// Image was not found in the registry
    #[error("image not found: {0}")]
    ImageNotFound(String),

    /// IO error during export
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Registry error
    #[error("registry error: {0}")]
    Registry(String),

    /// JSON serialization error
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Invalid image reference format
    #[error("invalid reference: {0}")]
    InvalidReference(String),
}

/// Errors that can occur during import operations
#[derive(Debug, Error)]
pub enum ImportError {
    /// The archive does not have a valid OCI Image Layout
    #[error("invalid OCI layout: {0}")]
    InvalidLayout(String),

    /// IO error during import
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Registry error
    #[error("registry error: {0}")]
    Registry(String),

    /// JSON deserialization error
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Blob referenced in manifest not found in archive
    #[error("blob not found in archive: {0}")]
    BlobNotFound(String),
}

/// Parse an image reference into name and reference (tag or digest)
///
/// Supports formats:
/// - `name:tag` -> ("name", "tag")
/// - `name@sha256:...` -> ("name", "sha256:...")
/// - `name` -> ("name", "latest")
fn parse_image_reference(image: &str) -> Result<(String, String), ExportError> {
    // Check for digest reference first
    if let Some(at_pos) = image.find('@') {
        let name = &image[..at_pos];
        let digest = &image[at_pos + 1..];
        if !digest.starts_with("sha256:") {
            return Err(ExportError::InvalidReference(format!(
                "digest must start with sha256: in '{}'",
                image
            )));
        }
        return Ok((name.to_string(), digest.to_string()));
    }

    // Check for tag reference
    if let Some(colon_pos) = image.rfind(':') {
        // Make sure it's not part of a port number or registry URL
        let potential_tag = &image[colon_pos + 1..];
        if !potential_tag.contains('/') && !potential_tag.is_empty() {
            let name = &image[..colon_pos];
            return Ok((name.to_string(), potential_tag.to_string()));
        }
    }

    // Default to latest tag
    Ok((image.to_string(), "latest".to_string()))
}

/// Export an image from the local registry to an OCI Image Layout tar archive
///
/// # Arguments
///
/// * `registry` - The local registry containing the image
/// * `image` - Image reference in the format `name:tag` or `name@sha256:...`
/// * `output` - Output path for the tar file. If it ends with `.gz`, the archive
///   will be gzip compressed.
///
/// # Returns
///
/// Returns `ExportInfo` containing details about the exported image.
///
/// # Example
///
/// ```rust,no_run
/// use zlayer_registry::oci_export::export_image;
/// use zlayer_registry::LocalRegistry;
/// use std::path::Path;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let registry = LocalRegistry::new("/var/lib/zlayer/registry".into()).await?;
/// let info = export_image(&registry, "myapp:v1.0", Path::new("/tmp/myapp.tar.gz")).await?;
/// println!("Exported image to {:?}", info.output_path);
/// # Ok(())
/// # }
/// ```
pub async fn export_image(
    registry: &LocalRegistry,
    image: &str,
    output: &Path,
) -> Result<ExportInfo, ExportError> {
    let (name, reference) = parse_image_reference(image)?;

    // Get the manifest
    let manifest_data = registry
        .get_manifest(&name, &reference)
        .await
        .map_err(|e| ExportError::Registry(e.to_string()))?;

    let manifest_digest = compute_digest(&manifest_data);

    // Parse the manifest to get layer and config digests
    let manifest: OciManifest = serde_json::from_slice(&manifest_data)?;

    // Collect all blobs we need to export
    let mut blobs_to_export: Vec<(String, Vec<u8>)> = Vec::new();

    // Add config blob if present
    if let Some(ref config) = manifest.config {
        let config_data = registry
            .get_blob(&config.digest)
            .await
            .map_err(|e| ExportError::Registry(e.to_string()))?;
        blobs_to_export.push((config.digest.clone(), config_data));
    }

    // Add layer blobs
    for layer in &manifest.layers {
        let layer_data = registry
            .get_blob(&layer.digest)
            .await
            .map_err(|e| ExportError::Registry(e.to_string()))?;
        blobs_to_export.push((layer.digest.clone(), layer_data));
    }

    // Add manifest blob
    blobs_to_export.push((manifest_digest.clone(), manifest_data.clone()));

    // Create OCI layout structure
    let oci_layout = OciLayout::default();
    let oci_layout_json = serde_json::to_string_pretty(&oci_layout)?;

    // Create index.json
    let mut annotations = HashMap::new();
    // Store original reference if it was a tag
    if !reference.starts_with("sha256:") {
        annotations.insert(
            "org.opencontainers.image.ref.name".to_string(),
            format!("{}:{}", name, reference),
        );
    }

    let manifest_descriptor = OciDescriptor {
        media_type: manifest
            .media_type
            .clone()
            .unwrap_or_else(|| "application/vnd.oci.image.manifest.v1+json".to_string()),
        digest: manifest_digest.clone(),
        size: manifest_data.len() as u64,
        urls: None,
        annotations: if annotations.is_empty() {
            None
        } else {
            Some(annotations)
        },
        platform: None,
    };

    let index = OciIndex::new(manifest_descriptor);
    let index_json = serde_json::to_string_pretty(&index)?;

    // Determine if we should compress
    let output_str = output.to_string_lossy();
    let compress = output_str.ends_with(".gz") || output_str.ends_with(".tgz");

    // Build the tar archive in memory first, then write to file
    let tar_data = {
        let mut tar_builder = Builder::new(Vec::new());

        // Add oci-layout file
        add_file_to_tar(&mut tar_builder, "oci-layout", oci_layout_json.as_bytes())?;

        // Add index.json
        add_file_to_tar(&mut tar_builder, "index.json", index_json.as_bytes())?;

        // Add all blobs
        for (digest, data) in &blobs_to_export {
            let hash = digest.strip_prefix("sha256:").unwrap_or(digest);
            let blob_path = format!("blobs/sha256/{}", hash);
            add_file_to_tar(&mut tar_builder, &blob_path, data)?;
        }

        tar_builder.finish()?;
        tar_builder.into_inner()?
    };

    // Write the tar (optionally compressed) to the output file
    let output_data = if compress {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar_data)?;
        encoder.finish()?
    } else {
        tar_data
    };

    fs::write(output, &output_data).await?;

    let export_info = ExportInfo {
        digest: manifest_digest,
        size: output_data.len() as u64,
        layers: manifest.layers.len(),
        output_path: output.to_path_buf(),
    };

    tracing::info!(
        digest = %export_info.digest,
        layers = export_info.layers,
        size = export_info.size,
        output = %export_info.output_path.display(),
        "exported image to OCI layout"
    );

    Ok(export_info)
}

/// Import an image from an OCI Image Layout tar archive into the local registry
///
/// # Arguments
///
/// * `registry` - The local registry to import into
/// * `input` - Path to the tar file (supports both `.tar` and `.tar.gz`)
/// * `tag` - Optional tag to apply to the imported image. If not provided, the
///   original tag from the archive annotations will be used if available.
///
/// # Returns
///
/// Returns `ImportInfo` containing details about the imported image.
///
/// # Example
///
/// ```rust,no_run
/// use zlayer_registry::oci_export::import_image;
/// use zlayer_registry::LocalRegistry;
/// use std::path::Path;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let registry = LocalRegistry::new("/var/lib/zlayer/registry".into()).await?;
/// let info = import_image(&registry, Path::new("/tmp/myapp.tar.gz"), Some("myapp:imported")).await?;
/// println!("Imported image with digest: {}", info.digest);
/// # Ok(())
/// # }
/// ```
pub async fn import_image(
    registry: &LocalRegistry,
    input: &Path,
    tag: Option<&str>,
) -> Result<ImportInfo, ImportError> {
    // Read the archive
    let archive_data = fs::read(input).await?;

    // Determine if it's compressed
    let input_str = input.to_string_lossy();
    let is_compressed =
        input_str.ends_with(".gz") || input_str.ends_with(".tgz") || is_gzip(&archive_data);

    // Decompress if needed
    let tar_data = if is_compressed {
        let mut decoder = GzDecoder::new(&archive_data[..]);
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed)?;
        decompressed
    } else {
        archive_data
    };

    // Extract archive contents into memory
    let mut archive = Archive::new(&tar_data[..]);
    let mut files: HashMap<String, Vec<u8>> = HashMap::new();

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().to_string();

        let mut contents = Vec::new();
        entry.read_to_end(&mut contents)?;
        files.insert(path, contents);
    }

    // Validate OCI layout
    let oci_layout_data = files
        .get("oci-layout")
        .ok_or_else(|| ImportError::InvalidLayout("missing oci-layout file".to_string()))?;

    let oci_layout: OciLayout = serde_json::from_slice(oci_layout_data)?;
    if oci_layout.image_layout_version != "1.0.0" {
        return Err(ImportError::InvalidLayout(format!(
            "unsupported OCI layout version: {}",
            oci_layout.image_layout_version
        )));
    }

    // Parse index.json
    let index_data = files
        .get("index.json")
        .ok_or_else(|| ImportError::InvalidLayout("missing index.json".to_string()))?;

    let index: OciIndex = serde_json::from_slice(index_data)?;

    if index.manifests.is_empty() {
        return Err(ImportError::InvalidLayout(
            "index.json contains no manifests".to_string(),
        ));
    }

    // For now, we import the first manifest
    let manifest_descriptor = &index.manifests[0];
    let manifest_digest = &manifest_descriptor.digest;

    // Get original reference from annotations if available
    let original_ref = manifest_descriptor
        .annotations
        .as_ref()
        .and_then(|a| a.get("org.opencontainers.image.ref.name"))
        .cloned();

    // Get manifest blob
    let manifest_hash = manifest_digest
        .strip_prefix("sha256:")
        .unwrap_or(manifest_digest);
    let manifest_path = format!("blobs/sha256/{}", manifest_hash);
    let manifest_data = files
        .get(&manifest_path)
        .ok_or_else(|| ImportError::BlobNotFound(manifest_digest.clone()))?;

    // Parse manifest
    let manifest: OciManifest = serde_json::from_slice(manifest_data)?;

    // Import all blobs (config and layers)
    let mut layer_count = 0;

    // Import config blob if present
    if let Some(ref config) = manifest.config {
        let config_hash = config
            .digest
            .strip_prefix("sha256:")
            .unwrap_or(&config.digest);
        let config_path = format!("blobs/sha256/{}", config_hash);
        let config_data = files
            .get(&config_path)
            .ok_or_else(|| ImportError::BlobNotFound(config.digest.clone()))?;

        registry
            .put_blob(config_data)
            .await
            .map_err(|e| ImportError::Registry(e.to_string()))?;
    }

    // Import layer blobs
    for layer in &manifest.layers {
        let layer_hash = layer
            .digest
            .strip_prefix("sha256:")
            .unwrap_or(&layer.digest);
        let layer_path = format!("blobs/sha256/{}", layer_hash);
        let layer_data = files
            .get(&layer_path)
            .ok_or_else(|| ImportError::BlobNotFound(layer.digest.clone()))?;

        registry
            .put_blob(layer_data)
            .await
            .map_err(|e| ImportError::Registry(e.to_string()))?;

        layer_count += 1;
    }

    // Determine the tag to use
    let (name, final_tag) = if let Some(tag_str) = tag {
        // Parse user-provided tag
        if let Some(at_pos) = tag_str.find('@') {
            // Digest reference - just use name, no tag
            (tag_str[..at_pos].to_string(), None)
        } else if let Some(colon_pos) = tag_str.rfind(':') {
            let potential_tag = &tag_str[colon_pos + 1..];
            if !potential_tag.contains('/') && !potential_tag.is_empty() {
                (
                    tag_str[..colon_pos].to_string(),
                    Some(potential_tag.to_string()),
                )
            } else {
                (tag_str.to_string(), Some("latest".to_string()))
            }
        } else {
            (tag_str.to_string(), Some("latest".to_string()))
        }
    } else if let Some(ref orig) = original_ref {
        // Use original reference from archive
        if let Some(colon_pos) = orig.rfind(':') {
            (
                orig[..colon_pos].to_string(),
                Some(orig[colon_pos + 1..].to_string()),
            )
        } else {
            (orig.clone(), Some("latest".to_string()))
        }
    } else {
        // Generate a name from the digest
        let short_digest = &manifest_digest[7..19]; // First 12 chars of hash
        (format!("imported-{}", short_digest), None)
    };

    // Store the manifest
    let reference = final_tag.clone().unwrap_or_else(|| manifest_digest.clone());
    registry
        .put_manifest(&name, &reference, manifest_data)
        .await
        .map_err(|e| ImportError::Registry(e.to_string()))?;

    let import_info = ImportInfo {
        digest: manifest_digest.clone(),
        tag: final_tag.map(|t| format!("{}:{}", name, t)),
        layers: layer_count,
    };

    tracing::info!(
        digest = %import_info.digest,
        tag = ?import_info.tag,
        layers = import_info.layers,
        "imported image from OCI layout"
    );

    Ok(import_info)
}

/// Add a file to a tar archive
fn add_file_to_tar<W: Write>(
    builder: &mut Builder<W>,
    path: &str,
    data: &[u8],
) -> Result<(), std::io::Error> {
    let mut header = tar::Header::new_gnu();
    header.set_path(path)?;
    header.set_size(data.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(0);
    header.set_cksum();

    builder.append(&header, data)?;
    Ok(())
}

/// Check if data appears to be gzip compressed
fn is_gzip(data: &[u8]) -> bool {
    data.len() >= 2 && data[0] == 0x1f && data[1] == 0x8b
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn create_test_registry() -> (LocalRegistry, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let registry = LocalRegistry::new(temp_dir.path().to_path_buf())
            .await
            .unwrap();
        (registry, temp_dir)
    }

    #[test]
    fn test_parse_image_reference() {
        // name:tag format
        let (name, reference) = parse_image_reference("myapp:v1.0").unwrap();
        assert_eq!(name, "myapp");
        assert_eq!(reference, "v1.0");

        // name only (defaults to latest)
        let (name, reference) = parse_image_reference("myapp").unwrap();
        assert_eq!(name, "myapp");
        assert_eq!(reference, "latest");

        // name@digest format
        let (name, reference) = parse_image_reference(
            "myapp@sha256:1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
        )
        .unwrap();
        assert_eq!(name, "myapp");
        assert_eq!(
            reference,
            "sha256:1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"
        );

        // namespaced image
        let (name, reference) = parse_image_reference("library/nginx:latest").unwrap();
        assert_eq!(name, "library/nginx");
        assert_eq!(reference, "latest");
    }

    #[test]
    fn test_oci_layout_default() {
        let layout = OciLayout::default();
        assert_eq!(layout.image_layout_version, "1.0.0");
    }

    #[test]
    fn test_oci_index_new() {
        let descriptor = OciDescriptor {
            media_type: "application/vnd.oci.image.manifest.v1+json".to_string(),
            digest: "sha256:abc123".to_string(),
            size: 1234,
            urls: None,
            annotations: None,
            platform: None,
        };

        let index = OciIndex::new(descriptor);
        assert_eq!(index.schema_version, 2);
        assert_eq!(index.manifests.len(), 1);
    }

    #[test]
    fn test_is_gzip() {
        // Gzip magic bytes
        assert!(is_gzip(&[0x1f, 0x8b, 0x08, 0x00]));

        // Not gzip
        assert!(!is_gzip(&[0x00, 0x00, 0x00, 0x00]));

        // Too short
        assert!(!is_gzip(&[0x1f]));
    }

    #[tokio::test]
    async fn test_export_import_roundtrip() {
        let (registry, temp_dir) = create_test_registry().await;

        // Create a simple image with one layer
        let layer_data = b"layer content data";
        let layer_digest = registry.put_blob(layer_data).await.unwrap();

        let config_data = br#"{"architecture":"amd64","os":"linux"}"#;
        let config_digest = registry.put_blob(config_data).await.unwrap();

        let manifest = serde_json::json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "config": {
                "mediaType": "application/vnd.oci.image.config.v1+json",
                "digest": config_digest,
                "size": config_data.len()
            },
            "layers": [{
                "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
                "digest": layer_digest,
                "size": layer_data.len()
            }]
        });

        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        registry
            .put_manifest("testapp", "v1.0", &manifest_bytes)
            .await
            .unwrap();

        // Export the image
        let export_path = temp_dir.path().join("export.tar.gz");
        let export_info = export_image(&registry, "testapp:v1.0", &export_path)
            .await
            .unwrap();

        assert_eq!(export_info.layers, 1);
        assert!(export_path.exists());

        // Create a new registry for import
        let (import_registry, _import_temp) = create_test_registry().await;

        // Import the image
        let import_info = import_image(&import_registry, &export_path, Some("imported:v1"))
            .await
            .unwrap();

        assert_eq!(import_info.layers, 1);
        assert_eq!(import_info.tag, Some("imported:v1".to_string()));

        // Verify the imported image exists
        assert!(import_registry.has_manifest("imported", "v1").await);
        assert!(import_registry.has_blob(&layer_digest).await);
        assert!(import_registry.has_blob(&config_digest).await);
    }

    #[tokio::test]
    async fn test_export_nonexistent_image() {
        let (registry, _temp_dir) = create_test_registry().await;

        let result =
            export_image(&registry, "nonexistent:latest", Path::new("/tmp/test.tar")).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_import_invalid_archive() {
        let (registry, temp_dir) = create_test_registry().await;

        // Create an invalid tar file
        let invalid_tar = temp_dir.path().join("invalid.tar");
        fs::write(&invalid_tar, b"not a valid tar file")
            .await
            .unwrap();

        let result = import_image(&registry, &invalid_tar, None).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_export_uncompressed() {
        let (registry, temp_dir) = create_test_registry().await;

        // Create a minimal image
        let layer_data = b"test layer";
        let layer_digest = registry.put_blob(layer_data).await.unwrap();

        let manifest = serde_json::json!({
            "schemaVersion": 2,
            "layers": [{
                "mediaType": "application/vnd.oci.image.layer.v1.tar",
                "digest": layer_digest,
                "size": layer_data.len()
            }]
        });

        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        registry
            .put_manifest("test", "latest", &manifest_bytes)
            .await
            .unwrap();

        // Export without compression
        let export_path = temp_dir.path().join("export.tar");
        let _export_info = export_image(&registry, "test:latest", &export_path)
            .await
            .unwrap();

        assert!(export_path.exists());

        // Verify it's not gzip compressed
        let data = fs::read(&export_path).await.unwrap();
        assert!(!is_gzip(&data));

        // Should still be importable - keep import_temp alive for the duration
        let (import_registry, _import_temp) = create_test_registry().await;
        let import_info = import_image(&import_registry, &export_path, Some("test:imported"))
            .await
            .unwrap();

        assert_eq!(import_info.layers, 1);
    }
}
