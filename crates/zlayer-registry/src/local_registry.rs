//! Local OCI registry for storing images built by zlayer
//!
//! This module implements a minimal OCI Distribution-compliant local registry
//! that stores images on the local filesystem. It can be used to store images
//! built locally before pushing to a remote registry.
//!
//! ## Directory Structure
//!
//! ```text
//! /var/lib/zlayer/registry/
//! |-- blobs/
//! |   |-- sha256/
//! |       |-- <digest>   # blob content
//! |-- manifests/
//! |   |-- <name>/
//! |       |-- tags/
//! |       |   |-- <tag>     # file containing manifest digest
//! |       |-- sha256/
//! |           |-- <digest>  # manifest content
//! |-- index.json         # index of all images
//! ```

use crate::cache::{compute_digest, validate_digest};
use crate::error::CacheError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs;
use tokio::sync::RwLock;

/// Result type alias for local registry operations
pub type Result<T> = std::result::Result<T, LocalRegistryError>;

/// Local registry errors
#[derive(Debug, thiserror::Error)]
pub enum LocalRegistryError {
    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Cache error (digest validation, etc.)
    #[error("cache error: {0}")]
    Cache(#[from] CacheError),

    /// Manifest not found
    #[error("manifest not found: {name}:{reference}")]
    ManifestNotFound { name: String, reference: String },

    /// Blob not found
    #[error("blob not found: {digest}")]
    BlobNotFound { digest: String },

    /// Image not found
    #[error("image not found: {name}")]
    ImageNotFound { name: String },

    /// Invalid reference format
    #[error("invalid reference format: {0}")]
    InvalidReference(String),

    /// JSON serialization error
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Index entry for a stored image
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageEntry {
    /// Image name (e.g., "myapp")
    pub name: String,
    /// Tags for this image
    pub tags: Vec<String>,
    /// Last modified timestamp (unix epoch seconds)
    pub updated_at: u64,
}

/// Local registry index stored in index.json
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RegistryIndex {
    /// Schema version for future compatibility
    pub schema_version: u32,
    /// All images in the registry
    pub images: HashMap<String, ImageEntry>,
}

impl RegistryIndex {
    /// Create a new empty index
    pub fn new() -> Self {
        Self {
            schema_version: 1,
            images: HashMap::new(),
        }
    }
}

/// Local OCI registry for storing images built by zlayer build
///
/// This provides a minimal OCI Distribution-compliant registry that stores
/// images on the local filesystem. It's designed for:
/// - Storing locally-built images before pushing to remote registries
/// - Caching images pulled from remote registries
/// - Development and testing workflows
///
/// ## Example
///
/// ```rust,no_run
/// use zlayer_registry::local_registry::LocalRegistry;
/// use std::path::PathBuf;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let registry = LocalRegistry::new(PathBuf::from("/var/lib/zlayer/registry")).await?;
///
/// // Store a blob
/// let data = b"layer data";
/// let digest = registry.put_blob(data).await?;
///
/// // Store a manifest
/// let manifest = br#"{"schemaVersion": 2, "layers": []}"#;
/// let manifest_digest = registry.put_manifest("myapp", "latest", manifest).await?;
///
/// // List tags
/// let tags = registry.list_tags("myapp").await?;
/// # Ok(())
/// # }
/// ```
pub struct LocalRegistry {
    /// Root directory for storing blobs and manifests
    root: PathBuf,
    /// In-memory index of tags to digests (cached from disk)
    index: RwLock<RegistryIndex>,
}

impl LocalRegistry {
    /// Create a new local registry at the given path
    ///
    /// Creates the directory structure if it doesn't exist.
    pub async fn new(root: PathBuf) -> Result<Self> {
        // Create directory structure
        let blobs_dir = root.join("blobs").join("sha256");
        let manifests_dir = root.join("manifests");

        fs::create_dir_all(&blobs_dir).await?;
        fs::create_dir_all(&manifests_dir).await?;

        // Load or create index
        let index_path = root.join("index.json");
        let index = if index_path.exists() {
            let content = fs::read_to_string(&index_path).await?;
            serde_json::from_str(&content)?
        } else {
            RegistryIndex::new()
        };

        Ok(Self {
            root,
            index: RwLock::new(index),
        })
    }

    /// Get the path to the blobs directory
    fn blobs_dir(&self) -> PathBuf {
        self.root.join("blobs").join("sha256")
    }

    /// Get the path to the manifests directory for a specific image
    fn manifests_dir(&self, name: &str) -> PathBuf {
        self.root.join("manifests").join(sanitize_name(name))
    }

    /// Get the path for a blob given its digest
    fn blob_path(&self, digest: &str) -> PathBuf {
        let hash = digest.strip_prefix("sha256:").unwrap_or(digest);
        self.blobs_dir().join(hash)
    }

    /// Get the path for a manifest given name and digest
    fn manifest_path(&self, name: &str, digest: &str) -> PathBuf {
        let hash = digest.strip_prefix("sha256:").unwrap_or(digest);
        self.manifests_dir(name).join("sha256").join(hash)
    }

    /// Get the path for a tag file
    fn tag_path(&self, name: &str, tag: &str) -> PathBuf {
        self.manifests_dir(name)
            .join("tags")
            .join(sanitize_tag(tag))
    }

    /// Save the index to disk
    async fn save_index(&self) -> Result<()> {
        let index = self.index.read().await;
        let content = serde_json::to_string_pretty(&*index)?;
        let index_path = self.root.join("index.json");
        fs::write(&index_path, content).await?;
        Ok(())
    }

    /// Store a blob and return its digest
    ///
    /// The digest is computed from the data using SHA-256.
    /// If the blob already exists, this is a no-op.
    pub async fn put_blob(&self, data: &[u8]) -> Result<String> {
        let digest = compute_digest(data);

        let blob_path = self.blob_path(&digest);

        // Check if already exists
        if blob_path.exists() {
            tracing::debug!(digest = %digest, "blob already exists");
            return Ok(digest);
        }

        // Write blob atomically
        let temp_path = blob_path.with_extension("tmp");
        fs::write(&temp_path, data).await?;
        fs::rename(&temp_path, &blob_path).await?;

        tracing::debug!(digest = %digest, size = data.len(), "stored blob");
        Ok(digest)
    }

    /// Get a blob by digest
    ///
    /// Returns the blob data if found, or an error if not found.
    pub async fn get_blob(&self, digest: &str) -> Result<Vec<u8>> {
        validate_digest(digest)?;

        let blob_path = self.blob_path(digest);

        if !blob_path.exists() {
            return Err(LocalRegistryError::BlobNotFound {
                digest: digest.to_string(),
            });
        }

        let data = fs::read(&blob_path).await?;

        // Verify integrity
        let actual_digest = compute_digest(&data);
        if actual_digest != digest {
            return Err(LocalRegistryError::Cache(CacheError::Corrupted(format!(
                "blob integrity check failed: expected {}, got {}",
                digest, actual_digest
            ))));
        }

        Ok(data)
    }

    /// Check if a blob exists
    pub async fn has_blob(&self, digest: &str) -> bool {
        if validate_digest(digest).is_err() {
            return false;
        }
        self.blob_path(digest).exists()
    }

    /// Store a manifest for an image
    ///
    /// If reference is a tag (not starting with "sha256:"), it creates a tag
    /// pointing to the manifest digest. Returns the manifest digest.
    pub async fn put_manifest(
        &self,
        name: &str,
        reference: &str,
        manifest: &[u8],
    ) -> Result<String> {
        let digest = compute_digest(manifest);

        // Create image directories
        let manifest_dir = self.manifests_dir(name).join("sha256");
        let tags_dir = self.manifests_dir(name).join("tags");
        fs::create_dir_all(&manifest_dir).await?;
        fs::create_dir_all(&tags_dir).await?;

        // Write manifest
        let manifest_path = self.manifest_path(name, &digest);
        if !manifest_path.exists() {
            let temp_path = manifest_path.with_extension("tmp");
            fs::write(&temp_path, manifest).await?;
            fs::rename(&temp_path, &manifest_path).await?;
            tracing::debug!(name = %name, digest = %digest, "stored manifest");
        }

        // Handle tag vs digest reference
        let is_tag = !reference.starts_with("sha256:");
        if is_tag {
            let tag_path = self.tag_path(name, reference);
            fs::write(&tag_path, &digest).await?;
            tracing::debug!(name = %name, tag = %reference, digest = %digest, "created tag");
        }

        // Update index
        {
            let mut index = self.index.write().await;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            let entry = index
                .images
                .entry(name.to_string())
                .or_insert_with(|| ImageEntry {
                    name: name.to_string(),
                    tags: Vec::new(),
                    updated_at: now,
                });

            if is_tag && !entry.tags.contains(&reference.to_string()) {
                entry.tags.push(reference.to_string());
            }
            entry.updated_at = now;
        }

        self.save_index().await?;

        Ok(digest)
    }

    /// Get a manifest by name and reference (tag or digest)
    pub async fn get_manifest(&self, name: &str, reference: &str) -> Result<Vec<u8>> {
        let digest = if reference.starts_with("sha256:") {
            reference.to_string()
        } else {
            // It's a tag, resolve to digest
            let tag_path = self.tag_path(name, reference);
            if !tag_path.exists() {
                return Err(LocalRegistryError::ManifestNotFound {
                    name: name.to_string(),
                    reference: reference.to_string(),
                });
            }
            fs::read_to_string(&tag_path).await?.trim().to_string()
        };

        let manifest_path = self.manifest_path(name, &digest);
        if !manifest_path.exists() {
            return Err(LocalRegistryError::ManifestNotFound {
                name: name.to_string(),
                reference: reference.to_string(),
            });
        }

        let data = fs::read(&manifest_path).await?;

        // Verify integrity
        let actual_digest = compute_digest(&data);
        if actual_digest != digest {
            return Err(LocalRegistryError::Cache(CacheError::Corrupted(format!(
                "manifest integrity check failed: expected {}, got {}",
                digest, actual_digest
            ))));
        }

        Ok(data)
    }

    /// Check if a manifest exists
    pub async fn has_manifest(&self, name: &str, reference: &str) -> bool {
        let digest = if reference.starts_with("sha256:") {
            reference.to_string()
        } else {
            let tag_path = self.tag_path(name, reference);
            match fs::read_to_string(&tag_path).await {
                Ok(d) => d.trim().to_string(),
                Err(_) => return false,
            }
        };

        self.manifest_path(name, &digest).exists()
    }

    /// List all tags for an image
    pub async fn list_tags(&self, name: &str) -> Result<Vec<String>> {
        let tags_dir = self.manifests_dir(name).join("tags");

        if !tags_dir.exists() {
            return Err(LocalRegistryError::ImageNotFound {
                name: name.to_string(),
            });
        }

        let mut tags = Vec::new();
        let mut entries = fs::read_dir(&tags_dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            if let Some(name) = entry.file_name().to_str() {
                tags.push(name.to_string());
            }
        }

        tags.sort();
        Ok(tags)
    }

    /// List all images in the registry
    pub async fn list_images(&self) -> Result<Vec<String>> {
        let index = self.index.read().await;
        let mut images: Vec<String> = index.images.keys().cloned().collect();
        images.sort();
        Ok(images)
    }

    /// Get image information
    pub async fn get_image_info(&self, name: &str) -> Result<ImageEntry> {
        let index = self.index.read().await;
        index
            .images
            .get(name)
            .cloned()
            .ok_or_else(|| LocalRegistryError::ImageNotFound {
                name: name.to_string(),
            })
    }

    /// Delete a tag from an image
    pub async fn delete_tag(&self, name: &str, tag: &str) -> Result<()> {
        let tag_path = self.tag_path(name, tag);

        if !tag_path.exists() {
            return Err(LocalRegistryError::ManifestNotFound {
                name: name.to_string(),
                reference: tag.to_string(),
            });
        }

        fs::remove_file(&tag_path).await?;

        // Update index
        {
            let mut index = self.index.write().await;
            if let Some(entry) = index.images.get_mut(name) {
                entry.tags.retain(|t| t != tag);
                if entry.tags.is_empty() {
                    index.images.remove(name);
                }
            }
        }

        self.save_index().await?;
        tracing::info!(name = %name, tag = %tag, "deleted tag");

        Ok(())
    }

    /// Delete an image and all its tags and manifests
    pub async fn delete_image(&self, name: &str) -> Result<()> {
        let manifests_dir = self.manifests_dir(name);

        if !manifests_dir.exists() {
            return Err(LocalRegistryError::ImageNotFound {
                name: name.to_string(),
            });
        }

        // Note: This doesn't delete blobs as they may be shared
        fs::remove_dir_all(&manifests_dir).await?;

        // Update index
        {
            let mut index = self.index.write().await;
            index.images.remove(name);
        }

        self.save_index().await?;
        tracing::info!(name = %name, "deleted image");

        Ok(())
    }

    /// Garbage collect unreferenced blobs
    ///
    /// This finds all blobs that are not referenced by any manifest
    /// and deletes them. Returns the number of bytes freed.
    pub async fn garbage_collect(&self) -> Result<u64> {
        // Collect all referenced digests from manifests
        let mut referenced: std::collections::HashSet<String> = std::collections::HashSet::new();

        let manifests_base = self.root.join("manifests");
        if manifests_base.exists() {
            let mut images = fs::read_dir(&manifests_base).await?;
            while let Some(image_entry) = images.next_entry().await? {
                let sha256_dir = image_entry.path().join("sha256");
                if sha256_dir.exists() {
                    let mut manifests = fs::read_dir(&sha256_dir).await?;
                    while let Some(manifest_entry) = manifests.next_entry().await? {
                        // Read manifest and extract layer digests
                        if let Ok(content) = fs::read(&manifest_entry.path()).await {
                            if let Ok(manifest) =
                                serde_json::from_slice::<serde_json::Value>(&content)
                            {
                                // Add config digest if present
                                if let Some(config) = manifest.get("config") {
                                    if let Some(digest) =
                                        config.get("digest").and_then(|d| d.as_str())
                                    {
                                        referenced.insert(digest.to_string());
                                    }
                                }
                                // Add layer digests
                                if let Some(layers) =
                                    manifest.get("layers").and_then(|l| l.as_array())
                                {
                                    for layer in layers {
                                        if let Some(digest) =
                                            layer.get("digest").and_then(|d| d.as_str())
                                        {
                                            referenced.insert(digest.to_string());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Find and delete unreferenced blobs
        let mut freed: u64 = 0;
        let blobs_dir = self.blobs_dir();

        if blobs_dir.exists() {
            let mut blobs = fs::read_dir(&blobs_dir).await?;
            while let Some(blob_entry) = blobs.next_entry().await? {
                let file_name = blob_entry.file_name();
                let digest = format!("sha256:{}", file_name.to_string_lossy());

                if !referenced.contains(&digest) {
                    if let Ok(metadata) = blob_entry.metadata().await {
                        freed += metadata.len();
                    }
                    fs::remove_file(blob_entry.path()).await?;
                    tracing::debug!(digest = %digest, "garbage collected blob");
                }
            }
        }

        if freed > 0 {
            tracing::info!(bytes_freed = freed, "garbage collection complete");
        }

        Ok(freed)
    }

    /// Get the total size of the registry in bytes
    pub async fn size(&self) -> Result<u64> {
        let mut total: u64 = 0;

        // Sum blob sizes
        let blobs_dir = self.blobs_dir();
        if blobs_dir.exists() {
            let mut blobs = fs::read_dir(&blobs_dir).await?;
            while let Some(entry) = blobs.next_entry().await? {
                if let Ok(metadata) = entry.metadata().await {
                    total += metadata.len();
                }
            }
        }

        // Sum manifest sizes
        let manifests_base = self.root.join("manifests");
        if manifests_base.exists() {
            total += dir_size_recursive(manifests_base.clone()).await?;
        }

        Ok(total)
    }
}

/// Sanitize an image name for use as a directory name
fn sanitize_name(name: &str) -> String {
    // Replace / with _ to flatten namespaces, keep alphanumeric and -._
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_' {
                c
            } else {
                // Replace / and any other special characters with _
                '_'
            }
        })
        .collect()
}

/// Sanitize a tag for use as a filename
fn sanitize_tag(tag: &str) -> String {
    // Tags are more restrictive, keep alphanumeric, -, ., _
    tag.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Recursively calculate directory size
fn dir_size_recursive(
    path: PathBuf,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<u64>> + Send>> {
    Box::pin(async move {
        let mut total: u64 = 0;
        let mut entries = fs::read_dir(&path).await?;

        while let Some(entry) = entries.next_entry().await? {
            let metadata = entry.metadata().await?;
            if metadata.is_dir() {
                total += dir_size_recursive(entry.path()).await?;
            } else {
                total += metadata.len();
            }
        }

        Ok(total)
    })
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

    #[tokio::test]
    async fn test_put_get_blob() {
        let (registry, _temp) = create_test_registry().await;

        let data = b"test blob data";
        let digest = registry.put_blob(data).await.unwrap();

        assert!(digest.starts_with("sha256:"));
        assert!(registry.has_blob(&digest).await);

        let retrieved = registry.get_blob(&digest).await.unwrap();
        assert_eq!(retrieved, data);
    }

    #[tokio::test]
    async fn test_put_get_manifest() {
        let (registry, _temp) = create_test_registry().await;

        let manifest = br#"{"schemaVersion": 2, "layers": []}"#;
        let digest = registry
            .put_manifest("myapp", "latest", manifest)
            .await
            .unwrap();

        assert!(digest.starts_with("sha256:"));
        assert!(registry.has_manifest("myapp", "latest").await);
        assert!(registry.has_manifest("myapp", &digest).await);

        // Get by tag
        let retrieved = registry.get_manifest("myapp", "latest").await.unwrap();
        assert_eq!(retrieved, manifest);

        // Get by digest
        let retrieved = registry.get_manifest("myapp", &digest).await.unwrap();
        assert_eq!(retrieved, manifest);
    }

    #[tokio::test]
    async fn test_list_tags() {
        let (registry, _temp) = create_test_registry().await;

        let manifest = br#"{"schemaVersion": 2, "layers": []}"#;
        registry
            .put_manifest("myapp", "v1.0.0", manifest)
            .await
            .unwrap();
        registry
            .put_manifest("myapp", "v1.1.0", manifest)
            .await
            .unwrap();
        registry
            .put_manifest("myapp", "latest", manifest)
            .await
            .unwrap();

        let tags = registry.list_tags("myapp").await.unwrap();
        assert_eq!(tags, vec!["latest", "v1.0.0", "v1.1.0"]);
    }

    #[tokio::test]
    async fn test_list_images() {
        let (registry, _temp) = create_test_registry().await;

        let manifest = br#"{"schemaVersion": 2, "layers": []}"#;
        registry
            .put_manifest("app1", "latest", manifest)
            .await
            .unwrap();
        registry
            .put_manifest("app2", "latest", manifest)
            .await
            .unwrap();

        let images = registry.list_images().await.unwrap();
        assert_eq!(images, vec!["app1", "app2"]);
    }

    #[tokio::test]
    async fn test_delete_tag() {
        let (registry, _temp) = create_test_registry().await;

        let manifest = br#"{"schemaVersion": 2, "layers": []}"#;
        registry
            .put_manifest("myapp", "v1.0.0", manifest)
            .await
            .unwrap();
        registry
            .put_manifest("myapp", "latest", manifest)
            .await
            .unwrap();

        registry.delete_tag("myapp", "v1.0.0").await.unwrap();

        let tags = registry.list_tags("myapp").await.unwrap();
        assert_eq!(tags, vec!["latest"]);
    }

    #[tokio::test]
    async fn test_delete_image() {
        let (registry, _temp) = create_test_registry().await;

        let manifest = br#"{"schemaVersion": 2, "layers": []}"#;
        registry
            .put_manifest("myapp", "latest", manifest)
            .await
            .unwrap();

        registry.delete_image("myapp").await.unwrap();

        let images = registry.list_images().await.unwrap();
        assert!(images.is_empty());
    }

    #[tokio::test]
    async fn test_garbage_collect() {
        let (registry, _temp) = create_test_registry().await;

        // Store some blobs
        let blob1 = b"blob 1 data";
        let digest1 = registry.put_blob(blob1).await.unwrap();

        let blob2 = b"unreferenced blob";
        let _digest2 = registry.put_blob(blob2).await.unwrap();

        // Store a manifest that references blob1
        let manifest = format!(
            r#"{{"schemaVersion": 2, "layers": [{{"digest": "{}", "size": {}}}]}}"#,
            digest1,
            blob1.len()
        );
        registry
            .put_manifest("myapp", "latest", manifest.as_bytes())
            .await
            .unwrap();

        // GC should remove blob2 but keep blob1
        let freed = registry.garbage_collect().await.unwrap();
        assert!(freed > 0);

        // blob1 should still exist
        assert!(registry.has_blob(&digest1).await);
    }

    #[test]
    fn test_sanitize_name() {
        assert_eq!(sanitize_name("myapp"), "myapp");
        assert_eq!(sanitize_name("my/app"), "my_app");
        assert_eq!(sanitize_name("my-app_v1.0"), "my-app_v1.0");
    }

    #[test]
    fn test_sanitize_tag() {
        assert_eq!(sanitize_tag("v1.0.0"), "v1.0.0");
        assert_eq!(sanitize_tag("latest"), "latest");
        assert_eq!(sanitize_tag("my-tag_1"), "my-tag_1");
    }
}
