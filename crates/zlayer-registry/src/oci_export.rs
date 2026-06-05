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
//! let info = import_image(&registry, Path::new("/tmp/myapp.tar.gz"), Some("myapp:imported"), None).await?;
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
    #[must_use]
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
                "digest must start with sha256: in '{image}'"
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
/// # Errors
///
/// Returns an error if the image reference is invalid, the manifest or blobs cannot be
/// read, or the output file cannot be written.
///
/// # Example
///
/// ```rust,no_run
/// use zlayer_registry::oci_export::export_image;
/// use zlayer_registry::LocalRegistry;
/// use std::path::Path;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let registry = LocalRegistry::new("/tmp/zlayer/registry".into()).await?;
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
            format!("{name}:{reference}"),
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
            let blob_path = format!("blobs/sha256/{hash}");
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
/// # Errors
///
/// Returns an error if the archive cannot be read, the OCI layout is invalid,
/// or blobs/manifests cannot be stored in the registry.
///
/// # Example
///
/// ```rust,no_run
/// use zlayer_registry::oci_export::import_image;
/// use zlayer_registry::LocalRegistry;
/// use std::path::Path;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let registry = LocalRegistry::new("/tmp/zlayer/registry".into()).await?;
/// let info = import_image(&registry, Path::new("/tmp/myapp.tar.gz"), Some("myapp:imported"), None).await?;
/// println!("Imported image with digest: {}", info.digest);
/// # Ok(())
/// # }
/// ```
pub async fn import_image(
    registry: &LocalRegistry,
    input: &Path,
    tag: Option<&str>,
    blob_cache: Option<&dyn crate::cache::BlobCacheBackend>,
) -> Result<ImportInfo, ImportError> {
    let archive_data = fs::read(input).await?;
    import_image_from_bytes(registry, archive_data, tag, blob_cache).await
}

/// Import an image from OCI tar archive bytes already resident in memory.
///
/// Same semantics as [`import_image`], but accepts an in-memory byte buffer
/// instead of reading from disk. Used by the `zlayer import --url` code path
/// (which fetches the archive over HTTP via [`crate::fetch_archive_from_url`])
/// and by any other caller that already has the archive bytes in hand.
///
/// Compression is auto-detected from the gzip magic number — no filename is
/// needed. Bytes may be a plain tar or a gzip-compressed tar.
///
/// # Errors
///
/// Returns [`ImportError`] on invalid OCI layout, missing/malformed manifest,
/// unknown blob references, or local registry I/O failures.
#[allow(clippy::too_many_lines)]
pub async fn import_image_from_bytes(
    registry: &LocalRegistry,
    archive_data: Vec<u8>,
    tag: Option<&str>,
    blob_cache: Option<&dyn crate::cache::BlobCacheBackend>,
) -> Result<ImportInfo, ImportError> {
    // Decompress if gzipped (magic-number detection, no filename required)
    let tar_data = if is_gzip(&archive_data) {
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

    // Dispatch on archive format. An OCI Image Layout is marked by an
    // `oci-layout` file; `docker save` / `podman save` instead emit a Docker
    // Archive whose entry point is a top-level `manifest.json` array (and which
    // has no `oci-layout`). Prefer OCI when both are present (hybrid archives).
    if files.contains_key("oci-layout") {
        import_from_oci_layout(registry, &files, tag, blob_cache).await
    } else if files.contains_key("manifest.json") {
        import_from_docker_archive(registry, &files, tag, blob_cache).await
    } else {
        Err(ImportError::InvalidLayout(
            "archive is neither an OCI image layout (no oci-layout) nor a Docker \
             archive (no manifest.json)"
                .to_string(),
        ))
    }
}

/// A single image entry in a Docker Archive `manifest.json` (the format produced
/// by `docker save` and `podman save --format docker-archive`).
#[derive(Debug, Clone, Deserialize)]
struct DockerManifestEntry {
    /// Path within the archive to the image config JSON (e.g.
    /// `blobs/sha256/<hash>` on modern engines, or `<hash>.json` on older ones).
    #[serde(rename = "Config")]
    config: String,
    /// Repository tags this image was saved under (e.g. `["nginx:latest"]`).
    #[serde(rename = "RepoTags")]
    repo_tags: Option<Vec<String>>,
    /// Ordered list of layer paths within the archive. Each entry may be an
    /// uncompressed `tar` (`<id>/layer.tar`) or a `blobs/sha256/<hash>` blob.
    #[serde(rename = "Layers", default)]
    layers: Vec<String>,
}

/// Look up a file in an extracted archive, tolerating a leading `./` on either
/// the stored tar path or the manifest-referenced path.
fn archive_lookup<'a>(files: &'a HashMap<String, Vec<u8>>, key: &str) -> Option<&'a Vec<u8>> {
    files
        .get(key)
        .or_else(|| files.get(key.trim_start_matches("./")))
        .or_else(|| files.get(&format!("./{key}")))
}

/// Resolve the `(name, tag)` to store an imported image under.
///
/// Precedence: an explicit user-supplied `tag`, then the archive's own
/// reference (OCI `ref.name` annotation or Docker `RepoTags[0]`), then a
/// generated `imported-<shortdigest>` name. A trailing `:<tag>` is only treated
/// as a tag when it is not actually a `registry:port` segment (i.e. the part
/// after the last colon contains no `/`).
fn resolve_name_and_tag(
    tag: Option<&str>,
    original_ref: Option<&str>,
    manifest_digest: &str,
) -> (String, Option<String>) {
    let split_ref = |r: &str| -> (String, Option<String>) {
        if let Some(at_pos) = r.find('@') {
            return (r[..at_pos].to_string(), None);
        }
        if let Some(colon_pos) = r.rfind(':') {
            let potential = &r[colon_pos + 1..];
            if !potential.contains('/') && !potential.is_empty() {
                return (r[..colon_pos].to_string(), Some(potential.to_string()));
            }
        }
        (r.to_string(), Some("latest".to_string()))
    };

    if let Some(t) = tag {
        split_ref(t)
    } else if let Some(orig) = original_ref {
        split_ref(orig)
    } else {
        // First 12 hex chars of the digest (skip the "sha256:" prefix).
        let short_digest = &manifest_digest[7..19];
        (format!("imported-{short_digest}"), None)
    }
}

/// Import an image from a Docker Archive (`docker save` / `podman save`).
///
/// Docker archives differ from OCI image layouts: their entry point is a
/// top-level `manifest.json` array referencing a config blob and an ordered set
/// of layer blobs (which may be uncompressed `tar` or gzip-compressed). We read
/// those blobs, synthesize an equivalent OCI image manifest over the bytes we
/// store (labelling each layer `tar` or `tar+gzip` by gzip magic), and persist
/// the config, layers, and manifest into the local registry + daemon blob cache
/// exactly as the OCI path does — so an imported Docker archive is afterwards
/// indistinguishable from a pulled image.
async fn import_from_docker_archive(
    registry: &LocalRegistry,
    files: &HashMap<String, Vec<u8>>,
    tag: Option<&str>,
    blob_cache: Option<&dyn crate::cache::BlobCacheBackend>,
) -> Result<ImportInfo, ImportError> {
    let manifest_json = files
        .get("manifest.json")
        .ok_or_else(|| ImportError::InvalidLayout("missing manifest.json".to_string()))?;

    let entries: Vec<DockerManifestEntry> = serde_json::from_slice(manifest_json)?;
    let entry = entries.into_iter().next().ok_or_else(|| {
        ImportError::InvalidLayout("manifest.json contains no images".to_string())
    })?;

    // Config blob.
    let config_data = archive_lookup(files, &entry.config)
        .ok_or_else(|| ImportError::BlobNotFound(entry.config.clone()))?;
    let config_digest = compute_digest(config_data);
    registry
        .put_blob(config_data)
        .await
        .map_err(|e| ImportError::Registry(e.to_string()))?;
    if let Some(cache) = blob_cache {
        let _ = cache.put(&config_digest, config_data).await;
    }
    let config_desc = OciDescriptor {
        media_type: "application/vnd.oci.image.config.v1+json".to_string(),
        digest: config_digest,
        size: config_data.len() as u64,
        urls: None,
        annotations: None,
        platform: None,
    };

    // Layer blobs (preserve manifest order).
    let mut layer_descs = Vec::with_capacity(entry.layers.len());
    for layer_ref in &entry.layers {
        let layer_data = archive_lookup(files, layer_ref)
            .ok_or_else(|| ImportError::BlobNotFound(layer_ref.clone()))?;
        let digest = compute_digest(layer_data);
        let media_type = if is_gzip(layer_data) {
            "application/vnd.oci.image.layer.v1.tar+gzip"
        } else {
            "application/vnd.oci.image.layer.v1.tar"
        };
        registry
            .put_blob(layer_data)
            .await
            .map_err(|e| ImportError::Registry(e.to_string()))?;
        if let Some(cache) = blob_cache {
            let _ = cache.put(&digest, layer_data).await;
        }
        layer_descs.push(OciDescriptor {
            media_type: media_type.to_string(),
            digest,
            size: layer_data.len() as u64,
            urls: None,
            annotations: None,
            platform: None,
        });
    }
    let layer_count = layer_descs.len();

    // Synthesize an OCI manifest over the stored bytes.
    let manifest = OciManifest {
        schema_version: 2,
        media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
        config: Some(config_desc),
        layers: layer_descs,
        annotations: None,
    };
    let manifest_data = serde_json::to_vec(&manifest)?;
    let manifest_digest = compute_digest(&manifest_data);

    let original_ref = entry
        .repo_tags
        .as_ref()
        .and_then(|tags| tags.first())
        .cloned();
    let (name, final_tag) = resolve_name_and_tag(tag, original_ref.as_deref(), &manifest_digest);

    let reference = final_tag.clone().unwrap_or_else(|| manifest_digest.clone());
    registry
        .put_manifest(&name, &reference, &manifest_data)
        .await
        .map_err(|e| ImportError::Registry(e.to_string()))?;

    if let Some(cache) = blob_cache {
        let image_ref = if let Some(ref t) = final_tag {
            format!("{name}:{t}")
        } else {
            name.clone()
        };
        let cache_key = crate::client::manifest_cache_key(&image_ref);
        let _ = cache.put(&cache_key, &manifest_data).await;
    }

    let import_info = ImportInfo {
        digest: manifest_digest.clone(),
        tag: final_tag.map(|t| format!("{name}:{t}")),
        layers: layer_count,
    };

    tracing::info!(
        digest = %import_info.digest,
        tag = ?import_info.tag,
        layers = import_info.layers,
        "imported image from Docker archive"
    );

    Ok(import_info)
}

/// Import an image from an OCI Image Layout archive (the format produced by
/// `zlayer export`, `skopeo copy ... oci-archive:`, etc.).
#[allow(clippy::too_many_lines)]
async fn import_from_oci_layout(
    registry: &LocalRegistry,
    files: &HashMap<String, Vec<u8>>,
    tag: Option<&str>,
    blob_cache: Option<&dyn crate::cache::BlobCacheBackend>,
) -> Result<ImportInfo, ImportError> {
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
    let manifest_path = format!("blobs/sha256/{manifest_hash}");
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
        let config_path = format!("blobs/sha256/{config_hash}");
        let config_data = files
            .get(&config_path)
            .ok_or_else(|| ImportError::BlobNotFound(config.digest.clone()))?;

        registry
            .put_blob(config_data)
            .await
            .map_err(|e| ImportError::Registry(e.to_string()))?;

        // Also populate the daemon's blob cache so `pull_image_config`
        // finds the config blob without going to the remote registry.
        // Without this, containers deployed with `pull_policy: never`
        // fall back to `/bin/sh` as their entrypoint because the image
        // config (with its `entrypoint`/`cmd`) is never loaded.
        if let Some(cache) = blob_cache {
            let _ = cache.put(&config.digest, config_data).await;
        }
    }

    // Import layer blobs
    for layer in &manifest.layers {
        let layer_hash = layer
            .digest
            .strip_prefix("sha256:")
            .unwrap_or(&layer.digest);
        let layer_path = format!("blobs/sha256/{layer_hash}");
        let layer_data = files
            .get(&layer_path)
            .ok_or_else(|| ImportError::BlobNotFound(layer.digest.clone()))?;

        registry
            .put_blob(layer_data)
            .await
            .map_err(|e| ImportError::Registry(e.to_string()))?;

        // Also populate the daemon's blob cache so ImagePuller finds
        // these layers without going to the remote registry.
        if let Some(cache) = blob_cache {
            let _ = cache.put(&layer.digest, layer_data).await;
        }

        layer_count += 1;
    }

    // Determine the name/tag to use (shared with the Docker-archive path).
    let (name, final_tag) = resolve_name_and_tag(tag, original_ref.as_deref(), manifest_digest);

    // Store the manifest
    let reference = final_tag.clone().unwrap_or_else(|| manifest_digest.clone());
    registry
        .put_manifest(&name, &reference, manifest_data)
        .await
        .map_err(|e| ImportError::Registry(e.to_string()))?;

    // Also populate the daemon's manifest cache so ImagePuller
    // finds this manifest without hitting the remote registry.
    // The cache key format must match client.rs pull_manifest():
    //   "manifest:{name}:{tag}"  (e.g. "manifest:zachhandley/zlayer-manager:latest")
    if let Some(cache) = blob_cache {
        let image_ref = if let Some(ref t) = final_tag {
            format!("{name}:{t}")
        } else {
            name.clone()
        };
        let cache_key = crate::client::manifest_cache_key(&image_ref);
        let _ = cache.put(&cache_key, manifest_data).await;
    }

    let import_info = ImportInfo {
        digest: manifest_digest.clone(),
        tag: final_tag.map(|t| format!("{name}:{t}")),
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
        let import_info = import_image(&import_registry, &export_path, Some("imported:v1"), None)
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

        let result = import_image(&registry, &invalid_tar, None, None).await;

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
        let import_info = import_image(&import_registry, &export_path, Some("test:imported"), None)
            .await
            .unwrap();

        assert_eq!(import_info.layers, 1);
    }

    /// Build an in-memory Docker Archive (the `docker save` / `podman save`
    /// layout): a top-level `manifest.json` array plus a config blob and the
    /// listed layer blobs, stored at legacy `<id>/layer.tar` style paths.
    fn build_docker_archive(repo_tags: &[&str], config: &[u8], layers: &[&[u8]]) -> Vec<u8> {
        let mut builder = Builder::new(Vec::new());
        add_file_to_tar(&mut builder, "config.json", config).unwrap();

        let mut layer_paths = Vec::new();
        for (i, layer) in layers.iter().enumerate() {
            let path = format!("layer{i}/layer.tar");
            add_file_to_tar(&mut builder, &path, layer).unwrap();
            layer_paths.push(path);
        }

        let manifest = serde_json::json!([{
            "Config": "config.json",
            "RepoTags": repo_tags,
            "Layers": layer_paths,
        }]);
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        add_file_to_tar(&mut builder, "manifest.json", &manifest_bytes).unwrap();

        builder.into_inner().unwrap()
    }

    #[tokio::test]
    async fn test_import_docker_archive_with_tag() {
        let (registry, _temp) = create_test_registry().await;

        let config =
            br#"{"architecture":"amd64","os":"linux","rootfs":{"type":"layers","diff_ids":["sha256:aaa"]}}"#;
        let layer = b"uncompressed layer tar bytes";
        let archive = build_docker_archive(&["nginx:latest"], config, &[layer]);

        let info = import_image_from_bytes(&registry, archive, Some("myimg:v1"), None)
            .await
            .expect("docker archive should import");

        assert_eq!(info.layers, 1);
        assert_eq!(info.tag.as_deref(), Some("myimg:v1"));

        // A synthesized OCI manifest is stored under the requested name:tag.
        let manifest_bytes = registry.get_manifest("myimg", "v1").await.unwrap();
        let manifest: OciManifest = serde_json::from_slice(&manifest_bytes).unwrap();
        assert_eq!(manifest.layers.len(), 1);
        // Uncompressed layer -> tar media type, digest over the stored bytes.
        assert_eq!(
            manifest.layers[0].media_type,
            "application/vnd.oci.image.layer.v1.tar"
        );
        assert_eq!(manifest.layers[0].digest, compute_digest(layer));

        // Config + layer blobs are retrievable from the registry.
        let cfg = manifest.config.expect("config descriptor");
        assert_eq!(cfg.digest, compute_digest(config));
        assert!(registry.get_blob(&cfg.digest).await.is_ok());
        assert!(registry.get_blob(&manifest.layers[0].digest).await.is_ok());
    }

    #[tokio::test]
    async fn test_import_docker_archive_repotags_fallback() {
        let (registry, _temp) = create_test_registry().await;

        let config = br#"{"architecture":"amd64","os":"linux"}"#;
        let layer = b"layer";
        // No explicit tag -> name/tag taken from RepoTags[0].
        let archive = build_docker_archive(&["library/nginx:1.27"], config, &[layer]);

        let info = import_image_from_bytes(&registry, archive, None, None)
            .await
            .unwrap();

        assert_eq!(info.tag.as_deref(), Some("library/nginx:1.27"));
        assert!(registry.get_manifest("library/nginx", "1.27").await.is_ok());
    }

    #[tokio::test]
    async fn test_import_docker_archive_gzipped_layer() {
        let (registry, _temp) = create_test_registry().await;

        // A gzip-compressed layer must be labelled tar+gzip in the manifest.
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(b"compressed layer contents").unwrap();
        let gz_layer = encoder.finish().unwrap();
        assert!(is_gzip(&gz_layer));

        let config = br#"{"os":"linux"}"#;
        let archive = build_docker_archive(&["x:y"], config, &[&gz_layer]);

        import_image_from_bytes(&registry, archive, Some("g:1"), None)
            .await
            .unwrap();

        let manifest: OciManifest =
            serde_json::from_slice(&registry.get_manifest("g", "1").await.unwrap()).unwrap();
        assert_eq!(
            manifest.layers[0].media_type,
            "application/vnd.oci.image.layer.v1.tar+gzip"
        );
    }

    #[tokio::test]
    async fn test_import_archive_without_oci_or_docker_manifest_errors() {
        let (registry, _temp) = create_test_registry().await;

        // A valid tar that is neither an OCI layout nor a Docker archive.
        let mut builder = Builder::new(Vec::new());
        add_file_to_tar(&mut builder, "random.txt", b"hello").unwrap();
        let archive = builder.into_inner().unwrap();

        let err = import_image_from_bytes(&registry, archive, None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, ImportError::InvalidLayout(_)));
    }
}
