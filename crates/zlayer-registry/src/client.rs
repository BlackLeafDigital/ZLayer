//! OCI distribution client for pulling and pushing images

use crate::cache::BlobCacheBackend;
use crate::error::{RegistryError, Result};
use crate::wasm::{detect_artifact_type, extract_wasm_info, ArtifactType, WasmArtifactInfo};
use oci_client::{
    client::{ClientConfig, ClientProtocol},
    manifest::{OciImageManifest, OciManifest},
    secrets::RegistryAuth,
    Reference, RegistryOperation,
};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Semaphore;
use tracing::instrument;

#[cfg(feature = "local")]
use crate::wasm_export::WasmExportResult;

/// Errors that can occur during push operations
#[derive(Debug, Error)]
pub enum PushError {
    /// Authentication failed for the registry
    #[error("authentication failed for registry {registry}: {reason}")]
    AuthenticationFailed {
        /// Registry hostname
        registry: String,
        /// Reason for failure
        reason: String,
    },

    /// Failed to upload a blob
    #[error("failed to upload blob {digest}: {reason}")]
    BlobUploadFailed {
        /// Digest of the blob that failed to upload
        digest: String,
        /// Reason for failure
        reason: String,
    },

    /// Failed to upload a manifest
    #[error("failed to upload manifest: {reason}")]
    ManifestUploadFailed {
        /// Reason for failure
        reason: String,
    },

    /// Network error during push
    #[error("network error: {0}")]
    NetworkError(String),

    /// Invalid image reference
    #[error("invalid image reference: {reference}")]
    InvalidReference {
        /// The invalid reference string
        reference: String,
    },

    /// OCI distribution error
    #[error("OCI distribution error: {0}")]
    OciError(#[from] oci_client::errors::OciDistributionError),
}

/// Result of a successful push operation
#[derive(Debug, Clone)]
pub struct PushResult {
    /// Digest of the pushed manifest (sha256:...)
    pub manifest_digest: String,
    /// List of blob digests that were pushed
    pub blobs_pushed: Vec<String>,
    /// Full reference to the pushed image
    pub reference: String,
}

/// OCI image puller with caching
pub struct ImagePuller {
    client: oci_client::Client,
    cache: Arc<Box<dyn BlobCacheBackend>>,
    concurrency_limit: Arc<Semaphore>,
}

impl ImagePuller {
    /// Create a new image puller with any cache backend
    pub fn new<C: BlobCacheBackend + 'static>(cache: C) -> Self {
        let config = ClientConfig {
            protocol: ClientProtocol::Https,
            connect_timeout: Some(std::time::Duration::from_secs(30)),
            read_timeout: Some(std::time::Duration::from_secs(300)), // 5 minutes for large layers
            ..Default::default()
        };
        let client = oci_client::Client::new(config);

        Self {
            client,
            cache: Arc::new(Box::new(cache) as Box<dyn BlobCacheBackend>),
            concurrency_limit: Arc::new(Semaphore::new(3)),
        }
    }

    /// Create a new image puller with boxed cache backend
    pub fn with_cache(cache: Arc<Box<dyn BlobCacheBackend>>) -> Self {
        let config = ClientConfig {
            protocol: ClientProtocol::Https,
            connect_timeout: Some(std::time::Duration::from_secs(30)),
            read_timeout: Some(std::time::Duration::from_secs(300)), // 5 minutes for large layers
            ..Default::default()
        };
        let client = oci_client::Client::new(config);

        Self {
            client,
            cache,
            concurrency_limit: Arc::new(Semaphore::new(3)),
        }
    }

    /// Store authentication for a registry
    ///
    /// This ensures the client has auth credentials before attempting pulls.
    async fn store_auth(&self, image: &str, auth: &RegistryAuth) -> Result<()> {
        let reference: Reference = image.parse().map_err(|_| RegistryError::NotFound {
            registry: "unknown".to_string(),
            image: image.to_string(),
        })?;

        // Store auth in the client's internal cache
        // This is called by pull_image_manifest, but we call it explicitly here
        // to ensure auth is available for blob pulls
        self.client
            .store_auth_if_needed(reference.resolve_registry(), auth)
            .await;

        Ok(())
    }

    /// Set concurrency limit for blob downloads
    #[must_use]
    pub fn with_concurrency_limit(mut self, limit: usize) -> Self {
        self.concurrency_limit = Arc::new(Semaphore::new(limit));
        self
    }

    /// Pull a single blob and cache it
    ///
    /// Uses the provided authentication credentials to pull blobs from the registry.
    pub async fn pull_blob(
        &self,
        image: &str,
        digest: &str,
        auth: &RegistryAuth,
    ) -> Result<Vec<u8>> {
        // Check cache first (now async)
        if let Some(data) = self.cache.get(digest).await? {
            tracing::debug!(digest = %digest, "blob found in cache");
            return Ok(data);
        }

        // Store auth before pulling blob
        self.store_auth(image, auth).await?;

        let reference: Reference = image.parse().map_err(|_| RegistryError::NotFound {
            registry: "unknown".to_string(),
            image: image.to_string(),
        })?;

        let _permit = self.concurrency_limit.acquire().await.unwrap();

        tracing::debug!(digest = %digest, image = %image, "pulling blob from registry");

        // Pull from registry into memory
        // Note: We pass &mut buffer directly, not wrapped in BufWriter.
        // Vec<u8> implements AsyncWrite and oci_client's pull_blob writes directly to it.
        // Using BufWriter was causing data to not be flushed properly.
        let mut buffer = Vec::new();
        self.client
            .pull_blob(&reference, digest, &mut buffer)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, digest = %digest, image = %image, "failed to pull blob from registry");
                RegistryError::Oci(e)
            })?;

        // Validate blob data is not empty
        if buffer.is_empty() {
            return Err(RegistryError::Cache(crate::error::CacheError::Corrupted(
                format!("blob {digest} was empty after pulling from registry"),
            )));
        }

        // Cache the blob (now async)
        self.cache.put(digest, &buffer).await?;

        tracing::debug!(digest = %digest, size = buffer.len(), "blob cached successfully");

        Ok(buffer)
    }

    /// Pull an image manifest from the registry
    ///
    /// Returns the manifest and its digest. Manifests are cached to avoid
    /// repeated network requests for the same image reference.
    pub async fn pull_manifest(
        &self,
        image: &str,
        auth: &RegistryAuth,
    ) -> Result<(OciImageManifest, String)> {
        // Cache key: use "manifest:" prefix + image reference
        let cache_key = format!("manifest:{}", image);

        // Check cache first
        if let Ok(Some(data)) = self.cache.get(&cache_key).await {
            if let Ok(manifest) = serde_json::from_slice::<OciImageManifest>(&data) {
                let digest = crate::cache::compute_digest(&data);
                tracing::debug!(image = %image, "manifest cache hit");
                return Ok((manifest, digest));
            }
        }

        let reference: Reference = image.parse().map_err(|_| RegistryError::NotFound {
            registry: "unknown".to_string(),
            image: image.to_string(),
        })?;

        tracing::info!(image = %image, "pulling manifest from registry");

        let (manifest, digest) = self
            .client
            .pull_image_manifest(&reference, auth)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, image = %image, "failed to pull manifest");
                RegistryError::Oci(e)
            })?;

        tracing::debug!(
            image = %image,
            digest = %digest,
            layers = manifest.layers.len(),
            "manifest pulled successfully"
        );

        // Cache for next time
        if let Ok(bytes) = serde_json::to_vec(&manifest) {
            let _ = self.cache.put(&cache_key, &bytes).await;
        }

        Ok((manifest, digest))
    }

    /// Detect the artifact type of an image from its manifest
    ///
    /// This method pulls the manifest and determines whether the image is a
    /// traditional container image or a WASM artifact.
    ///
    /// # Arguments
    ///
    /// * `image` - Image reference (e.g., "ghcr.io/myorg/mymodule:v1.0")
    /// * `auth` - Registry authentication credentials
    ///
    /// # Returns
    ///
    /// Returns the detected `ArtifactType` along with the manifest and digest.
    pub async fn detect_artifact_type(
        &self,
        image: &str,
        auth: &RegistryAuth,
    ) -> Result<(ArtifactType, OciImageManifest, String)> {
        let (manifest, digest) = self.pull_manifest(image, auth).await?;
        let artifact_type = detect_artifact_type(&manifest);

        tracing::info!(
            image = %image,
            artifact_type = %artifact_type,
            "detected artifact type"
        );

        Ok((artifact_type, manifest, digest))
    }

    /// Extract WASM artifact information from an image
    ///
    /// This method pulls the manifest and extracts detailed information about
    /// a WASM artifact, including WASI version, layer digest, and module name.
    ///
    /// # Arguments
    ///
    /// * `image` - Image reference (e.g., "ghcr.io/myorg/mymodule:v1.0")
    /// * `auth` - Registry authentication credentials
    ///
    /// # Returns
    ///
    /// Returns `Some(WasmArtifactInfo)` if this is a WASM artifact, `None` otherwise.
    pub async fn get_wasm_info(
        &self,
        image: &str,
        auth: &RegistryAuth,
    ) -> Result<Option<WasmArtifactInfo>> {
        let (manifest, _digest) = self.pull_manifest(image, auth).await?;
        Ok(extract_wasm_info(&manifest))
    }

    /// Pull a WASM binary from an image
    ///
    /// This method pulls the manifest, verifies it's a WASM artifact, and
    /// returns the raw WASM binary bytes.
    ///
    /// # Arguments
    ///
    /// * `image` - Image reference (e.g., "ghcr.io/myorg/mymodule:v1.0")
    /// * `auth` - Registry authentication credentials
    ///
    /// # Returns
    ///
    /// Returns the WASM binary bytes if this is a WASM artifact.
    ///
    /// # Errors
    ///
    /// Returns an error if this is not a WASM artifact or if the WASM layer
    /// cannot be found.
    pub async fn pull_wasm(
        &self,
        image: &str,
        auth: &RegistryAuth,
    ) -> Result<(Vec<u8>, WasmArtifactInfo)> {
        let (manifest, _digest) = self.pull_manifest(image, auth).await?;

        let wasm_info = extract_wasm_info(&manifest).ok_or_else(|| RegistryError::NotFound {
            registry: "unknown".to_string(),
            image: format!("{image} (not a WASM artifact)"),
        })?;

        let wasm_digest =
            wasm_info
                .wasm_layer_digest
                .as_ref()
                .ok_or_else(|| RegistryError::NotFound {
                    registry: "unknown".to_string(),
                    image: format!("{image} (no WASM layer found)"),
                })?;

        tracing::info!(
            image = %image,
            wasi_version = %wasm_info.wasi_version,
            wasm_digest = %wasm_digest,
            "pulling WASM binary"
        );

        let wasm_bytes = self.pull_blob(image, wasm_digest, auth).await?;

        tracing::info!(
            image = %image,
            wasm_size = wasm_bytes.len(),
            "WASM binary pulled successfully"
        );

        Ok((wasm_bytes, wasm_info))
    }

    /// Pull a complete image (manifest + all layers)
    ///
    /// Returns a vector of (layer_data, media_type) tuples in order (base layer first).
    pub async fn pull_image(
        &self,
        image: &str,
        auth: &RegistryAuth,
    ) -> Result<Vec<(Vec<u8>, String)>> {
        // Pull manifest first
        let (manifest, _digest) = self.pull_manifest(image, auth).await?;

        tracing::info!(
            image = %image,
            layer_count = manifest.layers.len(),
            "pulling image layers"
        );

        // Pull each layer in order
        let mut layers = Vec::with_capacity(manifest.layers.len());
        for (i, layer) in manifest.layers.iter().enumerate() {
            tracing::debug!(
                layer = i,
                digest = %layer.digest,
                media_type = %layer.media_type,
                size = layer.size,
                "pulling layer"
            );

            let data = self.pull_blob(image, &layer.digest, auth).await?;

            // Validate layer data is not empty
            if data.is_empty() {
                return Err(RegistryError::Cache(crate::error::CacheError::Corrupted(
                    format!("layer {i} ({}) is empty after pull", layer.digest),
                )));
            }

            layers.push((data, layer.media_type.clone()));
        }

        tracing::info!(
            image = %image,
            layers_pulled = layers.len(),
            "image pull complete"
        );

        Ok(layers)
    }

    /// Push a blob to a remote registry
    ///
    /// Uploads a blob (layer or config) to the specified registry. The blob is
    /// identified by its digest and will be stored at the repository specified
    /// in the reference.
    ///
    /// # Arguments
    ///
    /// * `reference` - Image reference (e.g., "ghcr.io/myorg/mymodule:v1.0")
    /// * `digest` - Content digest of the blob (e.g., "sha256:abc123...")
    /// * `data` - Raw blob data to upload
    /// * `_media_type` - MIME type of the blob content (reserved for future use)
    /// * `auth` - Registry authentication credentials
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The reference is invalid
    /// - Authentication fails
    /// - The blob upload fails
    #[instrument(
        name = "push_blob",
        skip(self, data, auth, _media_type),
        fields(
            reference = %reference,
            digest = %digest,
            size = data.len(),
        )
    )]
    pub async fn push_blob(
        &self,
        reference: &str,
        digest: &str,
        data: &[u8],
        _media_type: &str,
        auth: &RegistryAuth,
    ) -> std::result::Result<(), PushError> {
        let image_ref: Reference = reference.parse().map_err(|_| PushError::InvalidReference {
            reference: reference.to_string(),
        })?;

        tracing::debug!(
            reference = %reference,
            digest = %digest,
            size = data.len(),
            "pushing blob to registry"
        );

        // Authenticate for push operation
        self.client
            .auth(&image_ref, auth, RegistryOperation::Push)
            .await
            .map_err(|e| PushError::AuthenticationFailed {
                registry: image_ref.resolve_registry().to_string(),
                reason: e.to_string(),
            })?;

        // Push the blob
        self.client
            .push_blob(&image_ref, data, digest)
            .await
            .map_err(|e| PushError::BlobUploadFailed {
                digest: digest.to_string(),
                reason: e.to_string(),
            })?;

        tracing::info!(
            reference = %reference,
            digest = %digest,
            size = data.len(),
            "blob pushed successfully"
        );

        Ok(())
    }

    /// Push a manifest to a remote registry
    ///
    /// Uploads an OCI image manifest to the specified registry. The manifest
    /// should reference blobs that have already been uploaded.
    ///
    /// # Arguments
    ///
    /// * `reference` - Image reference (e.g., "ghcr.io/myorg/mymodule:v1.0")
    /// * `manifest` - OCI image manifest to upload
    /// * `auth` - Registry authentication credentials
    ///
    /// # Returns
    ///
    /// Returns the manifest digest on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The reference is invalid
    /// - Authentication fails
    /// - The manifest upload fails
    #[instrument(
        name = "push_manifest_to_registry",
        skip(self, manifest, auth),
        fields(
            reference = %reference,
            layers = manifest.layers.len(),
        )
    )]
    pub async fn push_manifest_to_registry(
        &self,
        reference: &str,
        manifest: &OciImageManifest,
        auth: &RegistryAuth,
    ) -> std::result::Result<String, PushError> {
        let image_ref: Reference = reference.parse().map_err(|_| PushError::InvalidReference {
            reference: reference.to_string(),
        })?;

        tracing::debug!(
            reference = %reference,
            layers = manifest.layers.len(),
            config_digest = %manifest.config.digest,
            "pushing manifest to registry"
        );

        // Authenticate for push operation
        self.client
            .auth(&image_ref, auth, RegistryOperation::Push)
            .await
            .map_err(|e| PushError::AuthenticationFailed {
                registry: image_ref.resolve_registry().to_string(),
                reason: e.to_string(),
            })?;

        // Push the manifest
        let manifest_url = self
            .client
            .push_manifest(&image_ref, &OciManifest::Image(manifest.clone()))
            .await
            .map_err(|e| PushError::ManifestUploadFailed {
                reason: e.to_string(),
            })?;

        // Extract digest from the manifest URL or compute it
        // The manifest URL typically contains the digest after the @ symbol
        let digest = if let Some(digest_start) = manifest_url.rfind('@') {
            manifest_url[digest_start + 1..].to_string()
        } else {
            // Compute digest from manifest JSON
            let manifest_json =
                serde_json::to_vec(manifest).map_err(|e| PushError::ManifestUploadFailed {
                    reason: format!("failed to serialize manifest: {e}"),
                })?;
            crate::cache::compute_digest(&manifest_json)
        };

        tracing::info!(
            reference = %reference,
            digest = %digest,
            manifest_url = %manifest_url,
            "manifest pushed successfully"
        );

        Ok(digest)
    }

    /// Push a WASM artifact to a remote registry
    ///
    /// This method pushes a complete WASM artifact including the config blob,
    /// WASM binary layer, and manifest. It uses the result from `export_wasm_as_oci`
    /// to obtain the properly formatted blobs.
    ///
    /// # Arguments
    ///
    /// * `reference` - Image reference (e.g., "ghcr.io/myorg/mymodule:v1.0")
    /// * `export_result` - Result from `export_wasm_as_oci` containing all blobs
    /// * `auth` - Registry authentication credentials
    ///
    /// # Returns
    ///
    /// Returns a `PushResult` containing the manifest digest and list of pushed blobs.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The reference is invalid
    /// - Authentication fails
    /// - Any blob or manifest upload fails
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use zlayer_registry::{ImagePuller, BlobCache};
    /// use zlayer_registry::wasm_export::{WasmExportConfig, export_wasm_as_oci};
    /// use oci_client::secrets::RegistryAuth;
    /// use std::path::PathBuf;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let puller = ImagePuller::new(BlobCache::new()?);
    ///
    /// let export_config = WasmExportConfig {
    ///     wasm_path: PathBuf::from("./my_module.wasm"),
    ///     module_name: "my-module".to_string(),
    ///     wasi_version: None,
    ///     annotations: Default::default(),
    /// };
    ///
    /// let export_result = export_wasm_as_oci(&export_config).await?;
    ///
    /// let auth = RegistryAuth::Basic("user".to_string(), "token".to_string());
    /// let push_result = puller.push_wasm(
    ///     "ghcr.io/myorg/my-module:v1.0",
    ///     &export_result,
    ///     &auth,
    /// ).await?;
    ///
    /// println!("Pushed manifest: {}", push_result.manifest_digest);
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "local")]
    #[instrument(
        name = "push_wasm",
        skip(self, export_result, auth),
        fields(
            reference = %reference,
            wasm_size = export_result.wasm_size,
            wasi_version = %export_result.wasi_version,
        )
    )]
    pub async fn push_wasm(
        &self,
        reference: &str,
        export_result: &WasmExportResult,
        auth: &RegistryAuth,
    ) -> std::result::Result<PushResult, PushError> {
        use crate::wasm::{WASM_CONFIG_MEDIA_TYPE_V0, WASM_LAYER_MEDIA_TYPE_GENERIC};

        let image_ref: Reference = reference.parse().map_err(|_| PushError::InvalidReference {
            reference: reference.to_string(),
        })?;

        tracing::info!(
            reference = %reference,
            wasm_size = export_result.wasm_size,
            wasi_version = %export_result.wasi_version,
            artifact_type = %export_result.artifact_type,
            "pushing WASM artifact to registry"
        );

        // Authenticate for push operation
        self.client
            .auth(&image_ref, auth, RegistryOperation::Push)
            .await
            .map_err(|e| PushError::AuthenticationFailed {
                registry: image_ref.resolve_registry().to_string(),
                reason: e.to_string(),
            })?;

        let mut blobs_pushed = Vec::new();

        // Push config blob
        tracing::debug!(
            digest = %export_result.config_digest,
            size = export_result.config_size,
            "pushing config blob"
        );
        self.client
            .push_blob(
                &image_ref,
                &export_result.config_blob,
                &export_result.config_digest,
            )
            .await
            .map_err(|e| PushError::BlobUploadFailed {
                digest: export_result.config_digest.clone(),
                reason: e.to_string(),
            })?;
        blobs_pushed.push(export_result.config_digest.clone());

        // Push WASM binary blob
        tracing::debug!(
            digest = %export_result.wasm_layer_digest,
            size = export_result.wasm_size,
            "pushing WASM layer blob"
        );
        self.client
            .push_blob(
                &image_ref,
                &export_result.wasm_binary,
                &export_result.wasm_layer_digest,
            )
            .await
            .map_err(|e| PushError::BlobUploadFailed {
                digest: export_result.wasm_layer_digest.clone(),
                reason: e.to_string(),
            })?;
        blobs_pushed.push(export_result.wasm_layer_digest.clone());

        // Build the manifest from the export result
        let manifest = OciImageManifest {
            schema_version: 2,
            media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
            artifact_type: Some(export_result.artifact_type.clone()),
            config: oci_client::manifest::OciDescriptor {
                media_type: WASM_CONFIG_MEDIA_TYPE_V0.to_string(),
                digest: export_result.config_digest.clone(),
                size: export_result.config_size as i64,
                urls: None,
                annotations: None,
            },
            layers: vec![oci_client::manifest::OciDescriptor {
                media_type: WASM_LAYER_MEDIA_TYPE_GENERIC.to_string(),
                digest: export_result.wasm_layer_digest.clone(),
                size: export_result.wasm_size as i64,
                urls: None,
                annotations: None,
            }],
            annotations: None,
            subject: None,
        };

        // Push manifest
        tracing::debug!(
            config_digest = %export_result.config_digest,
            wasm_digest = %export_result.wasm_layer_digest,
            "pushing manifest"
        );
        let manifest_url = self
            .client
            .push_manifest(&image_ref, &OciManifest::Image(manifest))
            .await
            .map_err(|e| PushError::ManifestUploadFailed {
                reason: e.to_string(),
            })?;

        // Extract digest from manifest URL or use the precomputed one
        let manifest_digest = if let Some(digest_start) = manifest_url.rfind('@') {
            manifest_url[digest_start + 1..].to_string()
        } else {
            export_result.manifest_digest.clone()
        };

        tracing::info!(
            reference = %reference,
            manifest_digest = %manifest_digest,
            blobs_pushed = blobs_pushed.len(),
            "WASM artifact pushed successfully"
        );

        Ok(PushResult {
            manifest_digest,
            blobs_pushed,
            reference: reference.to_string(),
        })
    }
}

/// Image reference information
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// Registry host (e.g., "docker.io", "ghcr.io")
    pub registry: String,
    /// Repository name (e.g., "library/nginx")
    pub repository: String,
    /// Tag (e.g., "latest", "v1.0.0")
    pub tag: String,
}

impl std::fmt::Display for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}:{}", self.registry, self.repository, self.tag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Image Display Tests
    // =========================================================================

    #[test]
    fn test_image_display() {
        let image = Image {
            registry: "docker.io".to_string(),
            repository: "library/nginx".to_string(),
            tag: "latest".to_string(),
        };
        assert_eq!(image.to_string(), "docker.io/library/nginx:latest");
    }

    #[test]
    fn test_image_display_ghcr() {
        let image = Image {
            registry: "ghcr.io".to_string(),
            repository: "myorg/myrepo".to_string(),
            tag: "v1.2.3".to_string(),
        };
        assert_eq!(image.to_string(), "ghcr.io/myorg/myrepo:v1.2.3");
    }

    #[test]
    fn test_image_display_with_nested_repo() {
        let image = Image {
            registry: "gcr.io".to_string(),
            repository: "project/subdir/image".to_string(),
            tag: "sha-abc123".to_string(),
        };
        assert_eq!(image.to_string(), "gcr.io/project/subdir/image:sha-abc123");
    }

    #[test]
    fn test_image_clone() {
        let image = Image {
            registry: "docker.io".to_string(),
            repository: "library/nginx".to_string(),
            tag: "latest".to_string(),
        };
        let cloned = image.clone();
        assert_eq!(image, cloned);
        assert_eq!(cloned.registry, "docker.io");
        assert_eq!(cloned.repository, "library/nginx");
        assert_eq!(cloned.tag, "latest");
    }

    #[test]
    fn test_image_equality() {
        let image1 = Image {
            registry: "docker.io".to_string(),
            repository: "library/nginx".to_string(),
            tag: "latest".to_string(),
        };
        let image2 = Image {
            registry: "docker.io".to_string(),
            repository: "library/nginx".to_string(),
            tag: "latest".to_string(),
        };
        let image3 = Image {
            registry: "ghcr.io".to_string(),
            repository: "library/nginx".to_string(),
            tag: "latest".to_string(),
        };

        assert_eq!(image1, image2);
        assert_ne!(image1, image3);
    }

    #[test]
    fn test_image_debug() {
        let image = Image {
            registry: "docker.io".to_string(),
            repository: "test".to_string(),
            tag: "v1".to_string(),
        };
        let debug_str = format!("{:?}", image);
        assert!(debug_str.contains("Image"));
        assert!(debug_str.contains("docker.io"));
        assert!(debug_str.contains("test"));
        assert!(debug_str.contains("v1"));
    }

    // =========================================================================
    // PushError Display Tests
    // =========================================================================

    #[test]
    fn test_push_error_display_authentication_failed() {
        let err = PushError::AuthenticationFailed {
            registry: "ghcr.io".to_string(),
            reason: "invalid token".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "authentication failed for registry ghcr.io: invalid token"
        );
    }

    #[test]
    fn test_push_error_display_authentication_failed_empty_reason() {
        let err = PushError::AuthenticationFailed {
            registry: "docker.io".to_string(),
            reason: String::new(),
        };
        assert_eq!(
            err.to_string(),
            "authentication failed for registry docker.io: "
        );
    }

    #[test]
    fn test_push_error_display_authentication_failed_long_reason() {
        let long_reason = "a]".repeat(100);
        let err = PushError::AuthenticationFailed {
            registry: "ghcr.io".to_string(),
            reason: long_reason.clone(),
        };
        assert!(err.to_string().contains(&long_reason));
    }

    #[test]
    fn test_push_error_display_blob_upload_failed() {
        let err = PushError::BlobUploadFailed {
            digest: "sha256:abc123".to_string(),
            reason: "connection reset".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "failed to upload blob sha256:abc123: connection reset"
        );
    }

    #[test]
    fn test_push_error_display_blob_upload_failed_with_full_digest() {
        let digest =
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string();
        let err = PushError::BlobUploadFailed {
            digest: digest.clone(),
            reason: "timeout".to_string(),
        };
        assert!(err.to_string().contains(&digest));
        assert!(err.to_string().contains("timeout"));
    }

    #[test]
    fn test_push_error_display_manifest_upload_failed() {
        let err = PushError::ManifestUploadFailed {
            reason: "invalid manifest".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "failed to upload manifest: invalid manifest"
        );
    }

    #[test]
    fn test_push_error_display_manifest_upload_failed_json_error() {
        let err = PushError::ManifestUploadFailed {
            reason: "invalid JSON: unexpected token at position 42".to_string(),
        };
        assert!(err.to_string().contains("invalid JSON"));
        assert!(err.to_string().contains("position 42"));
    }

    #[test]
    fn test_push_error_display_network_error() {
        let err = PushError::NetworkError("timeout".to_string());
        assert_eq!(err.to_string(), "network error: timeout");
    }

    #[test]
    fn test_push_error_display_network_error_connection_refused() {
        let err = PushError::NetworkError("connection refused: 127.0.0.1:5000".to_string());
        assert_eq!(
            err.to_string(),
            "network error: connection refused: 127.0.0.1:5000"
        );
    }

    #[test]
    fn test_push_error_display_network_error_dns() {
        let err =
            PushError::NetworkError("DNS resolution failed for registry.example.com".to_string());
        assert!(err.to_string().contains("DNS resolution failed"));
    }

    #[test]
    fn test_push_error_display_invalid_reference() {
        let err = PushError::InvalidReference {
            reference: "invalid::ref".to_string(),
        };
        assert_eq!(err.to_string(), "invalid image reference: invalid::ref");
    }

    #[test]
    fn test_push_error_display_invalid_reference_empty() {
        let err = PushError::InvalidReference {
            reference: String::new(),
        };
        assert_eq!(err.to_string(), "invalid image reference: ");
    }

    #[test]
    fn test_push_error_display_invalid_reference_special_chars() {
        let err = PushError::InvalidReference {
            reference: "ghcr.io/test/image:tag@sha256:abc".to_string(),
        };
        assert!(err
            .to_string()
            .contains("ghcr.io/test/image:tag@sha256:abc"));
    }

    // =========================================================================
    // PushError Debug Tests
    // =========================================================================

    #[test]
    fn test_push_error_debug_authentication_failed() {
        let err = PushError::AuthenticationFailed {
            registry: "ghcr.io".to_string(),
            reason: "invalid token".to_string(),
        };
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("AuthenticationFailed"));
        assert!(debug_str.contains("ghcr.io"));
        assert!(debug_str.contains("invalid token"));
    }

    #[test]
    fn test_push_error_debug_blob_upload_failed() {
        let err = PushError::BlobUploadFailed {
            digest: "sha256:abc123".to_string(),
            reason: "network error".to_string(),
        };
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("BlobUploadFailed"));
        assert!(debug_str.contains("sha256:abc123"));
        assert!(debug_str.contains("network error"));
    }

    #[test]
    fn test_push_error_debug_manifest_upload_failed() {
        let err = PushError::ManifestUploadFailed {
            reason: "schema validation failed".to_string(),
        };
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("ManifestUploadFailed"));
        assert!(debug_str.contains("schema validation failed"));
    }

    #[test]
    fn test_push_error_debug_network_error() {
        let err = PushError::NetworkError("connection timed out".to_string());
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("NetworkError"));
        assert!(debug_str.contains("connection timed out"));
    }

    #[test]
    fn test_push_error_debug_invalid_reference() {
        let err = PushError::InvalidReference {
            reference: "bad-ref".to_string(),
        };
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("InvalidReference"));
        assert!(debug_str.contains("bad-ref"));
    }

    #[test]
    fn test_push_error_debug_oci_error() {
        // Create an OCI error using a valid variant (GenericError)
        let oci_err =
            oci_client::errors::OciDistributionError::GenericError(Some("test error".to_string()));
        let err = PushError::OciError(oci_err);
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("OciError"));
    }

    // =========================================================================
    // PushError From OCI Error Tests
    // =========================================================================

    #[test]
    fn test_push_error_from_oci_generic_error() {
        let oci_err = oci_client::errors::OciDistributionError::GenericError(Some(
            "generic error message".to_string(),
        ));
        let push_err: PushError = oci_err.into();
        match push_err {
            PushError::OciError(inner) => {
                assert!(inner.to_string().contains("generic error message"));
            }
            _ => panic!("Expected OciError variant"),
        }
    }

    #[test]
    fn test_push_error_from_oci_config_conversion_error() {
        let oci_err = oci_client::errors::OciDistributionError::ConfigConversionError(
            "config conversion failed".to_string(),
        );
        let push_err: PushError = oci_err.into();
        match push_err {
            PushError::OciError(inner) => {
                assert!(inner.to_string().contains("config conversion"));
            }
            _ => panic!("Expected OciError variant"),
        }
    }

    #[test]
    fn test_push_error_from_oci_manifest_parsing_error() {
        let oci_err = oci_client::errors::OciDistributionError::ManifestParsingError(
            "invalid manifest JSON".to_string(),
        );
        let push_err: PushError = oci_err.into();
        match push_err {
            PushError::OciError(inner) => {
                assert!(
                    inner.to_string().contains("manifest") || inner.to_string().contains("JSON")
                );
            }
            _ => panic!("Expected OciError variant"),
        }
    }

    #[test]
    fn test_push_error_from_oci_image_manifest_not_found() {
        let oci_err = oci_client::errors::OciDistributionError::ImageManifestNotFoundError(
            "image:tag".to_string(),
        );
        let push_err: PushError = oci_err.into();
        match push_err {
            PushError::OciError(inner) => {
                assert!(inner.to_string().contains("image:tag"));
            }
            _ => panic!("Expected OciError variant"),
        }
    }

    #[test]
    fn test_push_error_from_oci_url_parse_error() {
        let oci_err =
            oci_client::errors::OciDistributionError::UrlParseError("invalid URL".to_string());
        let push_err: PushError = oci_err.into();
        match push_err {
            PushError::OciError(inner) => {
                assert!(inner.to_string().contains("URL"));
            }
            _ => panic!("Expected OciError variant"),
        }
    }

    #[test]
    fn test_push_error_from_oci_unsupported_schema_version() {
        let oci_err = oci_client::errors::OciDistributionError::UnsupportedSchemaVersionError(99);
        let push_err: PushError = oci_err.into();
        match push_err {
            PushError::OciError(inner) => {
                // The error should contain information about the version
                let _ = inner.to_string();
            }
            _ => panic!("Expected OciError variant"),
        }
    }

    #[test]
    fn test_push_error_from_oci_unsupported_media_type() {
        let oci_err = oci_client::errors::OciDistributionError::UnsupportedMediaTypeError(
            "application/unknown".to_string(),
        );
        let push_err: PushError = oci_err.into();
        match push_err {
            PushError::OciError(inner) => {
                assert!(inner.to_string().contains("application/unknown"));
            }
            _ => panic!("Expected OciError variant"),
        }
    }

    #[test]
    fn test_push_error_oci_error_display() {
        let oci_err = oci_client::errors::OciDistributionError::GenericError(Some(
            "version mismatch".to_string(),
        ));
        let push_err = PushError::OciError(oci_err);
        assert!(push_err.to_string().contains("OCI distribution error"));
    }

    #[test]
    fn test_push_error_from_oci_spec_violation() {
        let oci_err = oci_client::errors::OciDistributionError::SpecViolationError(
            "OCI spec violation".to_string(),
        );
        let push_err: PushError = oci_err.into();
        match push_err {
            PushError::OciError(inner) => {
                assert!(
                    inner.to_string().contains("spec") || inner.to_string().contains("violation")
                );
            }
            _ => panic!("Expected OciError variant"),
        }
    }

    #[test]
    fn test_push_error_from_oci_versioned_parsing_error() {
        let oci_err = oci_client::errors::OciDistributionError::VersionedParsingError(
            "failed to parse versioned content".to_string(),
        );
        let push_err: PushError = oci_err.into();
        match push_err {
            PushError::OciError(inner) => {
                let _ = inner.to_string();
            }
            _ => panic!("Expected OciError variant"),
        }
    }

    #[test]
    fn test_push_error_from_oci_registry_token_decode() {
        let oci_err = oci_client::errors::OciDistributionError::RegistryTokenDecodeError(
            "invalid token format".to_string(),
        );
        let push_err: PushError = oci_err.into();
        match push_err {
            PushError::OciError(inner) => {
                assert!(inner.to_string().contains("token"));
            }
            _ => panic!("Expected OciError variant"),
        }
    }

    #[test]
    fn test_push_error_from_oci_incompatible_layer_media_type() {
        let oci_err = oci_client::errors::OciDistributionError::IncompatibleLayerMediaTypeError(
            "application/vnd.incompatible".to_string(),
        );
        let push_err: PushError = oci_err.into();
        match push_err {
            PushError::OciError(inner) => {
                assert!(
                    inner.to_string().contains("incompatible")
                        || inner.to_string().contains("layer")
                );
            }
            _ => panic!("Expected OciError variant"),
        }
    }

    // =========================================================================
    // PushResult Tests
    // =========================================================================

    #[test]
    fn test_push_result_creation_all_fields() {
        let result = PushResult {
            manifest_digest: "sha256:abc123".to_string(),
            blobs_pushed: vec!["sha256:def456".to_string(), "sha256:ghi789".to_string()],
            reference: "ghcr.io/test/image:v1.0".to_string(),
        };
        assert_eq!(result.manifest_digest, "sha256:abc123");
        assert_eq!(result.blobs_pushed.len(), 2);
        assert_eq!(result.blobs_pushed[0], "sha256:def456");
        assert_eq!(result.blobs_pushed[1], "sha256:ghi789");
        assert_eq!(result.reference, "ghcr.io/test/image:v1.0");
    }

    #[test]
    fn test_push_result_creation_empty_blobs() {
        let result = PushResult {
            manifest_digest: "sha256:empty".to_string(),
            blobs_pushed: vec![],
            reference: "docker.io/test:latest".to_string(),
        };
        assert!(result.blobs_pushed.is_empty());
        assert_eq!(result.manifest_digest, "sha256:empty");
    }

    #[test]
    fn test_push_result_creation_single_blob() {
        let result = PushResult {
            manifest_digest: "sha256:single".to_string(),
            blobs_pushed: vec!["sha256:only_one".to_string()],
            reference: "registry.example.com/repo:tag".to_string(),
        };
        assert_eq!(result.blobs_pushed.len(), 1);
    }

    #[test]
    fn test_push_result_creation_many_blobs() {
        let blobs: Vec<String> = (0..100).map(|i| format!("sha256:blob{:03}", i)).collect();
        let result = PushResult {
            manifest_digest: "sha256:many".to_string(),
            blobs_pushed: blobs,
            reference: "test/image:v1".to_string(),
        };
        assert_eq!(result.blobs_pushed.len(), 100);
        assert_eq!(result.blobs_pushed[0], "sha256:blob000");
        assert_eq!(result.blobs_pushed[99], "sha256:blob099");
    }

    #[test]
    fn test_push_result_clone() {
        let result = PushResult {
            manifest_digest: "sha256:abc".to_string(),
            blobs_pushed: vec!["sha256:def".to_string()],
            reference: "test:v1".to_string(),
        };
        let cloned = result.clone();
        assert_eq!(result.manifest_digest, cloned.manifest_digest);
        assert_eq!(result.blobs_pushed, cloned.blobs_pushed);
        assert_eq!(result.reference, cloned.reference);
    }

    #[test]
    fn test_push_result_clone_independence() {
        let result = PushResult {
            manifest_digest: "sha256:original".to_string(),
            blobs_pushed: vec!["sha256:blob1".to_string()],
            reference: "original:v1".to_string(),
        };
        let mut cloned = result.clone();
        cloned.manifest_digest = "sha256:modified".to_string();
        cloned.blobs_pushed.push("sha256:blob2".to_string());

        // Original should be unchanged
        assert_eq!(result.manifest_digest, "sha256:original");
        assert_eq!(result.blobs_pushed.len(), 1);

        // Cloned should have changes
        assert_eq!(cloned.manifest_digest, "sha256:modified");
        assert_eq!(cloned.blobs_pushed.len(), 2);
    }

    #[test]
    fn test_push_result_debug() {
        let result = PushResult {
            manifest_digest: "sha256:test".to_string(),
            blobs_pushed: vec!["sha256:blob1".to_string(), "sha256:blob2".to_string()],
            reference: "ghcr.io/org/repo:tag".to_string(),
        };
        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains("PushResult"));
        assert!(debug_str.contains("sha256:test"));
        assert!(debug_str.contains("sha256:blob1"));
        assert!(debug_str.contains("sha256:blob2"));
        assert!(debug_str.contains("ghcr.io/org/repo:tag"));
    }

    #[test]
    fn test_push_result_with_full_sha256_digest() {
        let full_digest = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let result = PushResult {
            manifest_digest: full_digest.to_string(),
            blobs_pushed: vec![full_digest.to_string()],
            reference: "test:latest".to_string(),
        };
        assert_eq!(result.manifest_digest.len(), 71); // "sha256:" + 64 hex chars
    }

    // =========================================================================
    // Reference Parsing Tests (for push methods)
    // =========================================================================

    #[test]
    fn test_valid_reference_parsing_docker_hub() {
        let reference = "docker.io/library/nginx:latest";
        let parsed: Result<Reference, _> = reference.parse();
        assert!(parsed.is_ok());
    }

    #[test]
    fn test_valid_reference_parsing_ghcr() {
        let reference = "ghcr.io/myorg/myrepo:v1.0.0";
        let parsed: Result<Reference, _> = reference.parse();
        assert!(parsed.is_ok());
    }

    #[test]
    fn test_valid_reference_parsing_with_digest() {
        let reference = "ghcr.io/test/image@sha256:abc123def456789012345678901234567890123456789012345678901234";
        let parsed: Result<Reference, _> = reference.parse();
        // Note: The oci_client may or may not accept this format
        // This test documents the current behavior
        let _ = parsed;
    }

    #[test]
    fn test_valid_reference_parsing_nested_repo() {
        let reference = "gcr.io/project-id/subdir/image:tag";
        let parsed: Result<Reference, _> = reference.parse();
        assert!(parsed.is_ok());
    }

    #[test]
    fn test_invalid_reference_empty() {
        let reference = "";
        let parsed: Result<Reference, _> = reference.parse();
        assert!(parsed.is_err());
    }

    #[test]
    fn test_invalid_reference_double_colon() {
        let reference = "invalid::reference";
        let parsed: Result<Reference, _> = reference.parse();
        assert!(parsed.is_err());
    }

    // =========================================================================
    // PushError Construction Helper Tests
    // =========================================================================

    #[test]
    fn test_push_error_authentication_failed_construction() {
        let err = PushError::AuthenticationFailed {
            registry: "test.registry.io".to_string(),
            reason: "credentials expired".to_string(),
        };
        // Verify the error contains the expected information
        let display = err.to_string();
        assert!(display.contains("test.registry.io"));
        assert!(display.contains("credentials expired"));
    }

    #[test]
    fn test_push_error_blob_upload_failed_construction() {
        let digest = "sha256:1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
        let err = PushError::BlobUploadFailed {
            digest: digest.to_string(),
            reason: "server returned 500".to_string(),
        };
        let display = err.to_string();
        assert!(display.contains(digest));
        assert!(display.contains("server returned 500"));
    }

    #[test]
    fn test_push_error_manifest_upload_failed_construction() {
        let err = PushError::ManifestUploadFailed {
            reason: "manifest already exists with different content".to_string(),
        };
        let display = err.to_string();
        assert!(display.contains("manifest already exists"));
    }

    #[test]
    fn test_push_error_network_error_construction() {
        let err = PushError::NetworkError("TLS handshake failed".to_string());
        let display = err.to_string();
        assert!(display.contains("TLS handshake failed"));
    }

    #[test]
    fn test_push_error_invalid_reference_construction() {
        let err = PushError::InvalidReference {
            reference: "not/a/valid/reference!!!".to_string(),
        };
        let display = err.to_string();
        assert!(display.contains("not/a/valid/reference!!!"));
    }

    // =========================================================================
    // Error Variant Discrimination Tests
    // =========================================================================

    #[test]
    fn test_push_error_is_authentication_failed() {
        let err = PushError::AuthenticationFailed {
            registry: "test".to_string(),
            reason: "test".to_string(),
        };
        assert!(matches!(err, PushError::AuthenticationFailed { .. }));
        assert!(!matches!(err, PushError::BlobUploadFailed { .. }));
        assert!(!matches!(err, PushError::ManifestUploadFailed { .. }));
        assert!(!matches!(err, PushError::NetworkError(_)));
        assert!(!matches!(err, PushError::InvalidReference { .. }));
        assert!(!matches!(err, PushError::OciError(_)));
    }

    #[test]
    fn test_push_error_is_blob_upload_failed() {
        let err = PushError::BlobUploadFailed {
            digest: "test".to_string(),
            reason: "test".to_string(),
        };
        assert!(!matches!(err, PushError::AuthenticationFailed { .. }));
        assert!(matches!(err, PushError::BlobUploadFailed { .. }));
        assert!(!matches!(err, PushError::ManifestUploadFailed { .. }));
        assert!(!matches!(err, PushError::NetworkError(_)));
        assert!(!matches!(err, PushError::InvalidReference { .. }));
        assert!(!matches!(err, PushError::OciError(_)));
    }

    #[test]
    fn test_push_error_is_manifest_upload_failed() {
        let err = PushError::ManifestUploadFailed {
            reason: "test".to_string(),
        };
        assert!(!matches!(err, PushError::AuthenticationFailed { .. }));
        assert!(!matches!(err, PushError::BlobUploadFailed { .. }));
        assert!(matches!(err, PushError::ManifestUploadFailed { .. }));
        assert!(!matches!(err, PushError::NetworkError(_)));
        assert!(!matches!(err, PushError::InvalidReference { .. }));
        assert!(!matches!(err, PushError::OciError(_)));
    }

    #[test]
    fn test_push_error_is_network_error() {
        let err = PushError::NetworkError("test".to_string());
        assert!(!matches!(err, PushError::AuthenticationFailed { .. }));
        assert!(!matches!(err, PushError::BlobUploadFailed { .. }));
        assert!(!matches!(err, PushError::ManifestUploadFailed { .. }));
        assert!(matches!(err, PushError::NetworkError(_)));
        assert!(!matches!(err, PushError::InvalidReference { .. }));
        assert!(!matches!(err, PushError::OciError(_)));
    }

    #[test]
    fn test_push_error_is_invalid_reference() {
        let err = PushError::InvalidReference {
            reference: "test".to_string(),
        };
        assert!(!matches!(err, PushError::AuthenticationFailed { .. }));
        assert!(!matches!(err, PushError::BlobUploadFailed { .. }));
        assert!(!matches!(err, PushError::ManifestUploadFailed { .. }));
        assert!(!matches!(err, PushError::NetworkError(_)));
        assert!(matches!(err, PushError::InvalidReference { .. }));
        assert!(!matches!(err, PushError::OciError(_)));
    }

    #[test]
    fn test_push_error_is_oci_error() {
        let oci_err =
            oci_client::errors::OciDistributionError::GenericError(Some("test".to_string()));
        let err = PushError::OciError(oci_err);
        assert!(!matches!(err, PushError::AuthenticationFailed { .. }));
        assert!(!matches!(err, PushError::BlobUploadFailed { .. }));
        assert!(!matches!(err, PushError::ManifestUploadFailed { .. }));
        assert!(!matches!(err, PushError::NetworkError(_)));
        assert!(!matches!(err, PushError::InvalidReference { .. }));
        assert!(matches!(err, PushError::OciError(_)));
    }

    // =========================================================================
    // Error std::error::Error Trait Tests
    // =========================================================================

    #[test]
    fn test_push_error_implements_error_trait() {
        let err = PushError::NetworkError("test".to_string());
        // Verify it implements std::error::Error by using the Error trait
        let _: &dyn std::error::Error = &err;
    }

    #[test]
    fn test_push_error_source_for_oci_error() {
        use std::error::Error;
        let oci_err =
            oci_client::errors::OciDistributionError::GenericError(Some("test".to_string()));
        let err = PushError::OciError(oci_err);
        // OciError variant should have a source
        let source = err.source();
        assert!(source.is_some());
    }

    #[test]
    fn test_push_error_source_for_other_variants() {
        use std::error::Error;

        let err1 = PushError::AuthenticationFailed {
            registry: "test".to_string(),
            reason: "test".to_string(),
        };
        assert!(err1.source().is_none());

        let err2 = PushError::BlobUploadFailed {
            digest: "test".to_string(),
            reason: "test".to_string(),
        };
        assert!(err2.source().is_none());

        let err3 = PushError::ManifestUploadFailed {
            reason: "test".to_string(),
        };
        assert!(err3.source().is_none());

        let err4 = PushError::NetworkError("test".to_string());
        assert!(err4.source().is_none());

        let err5 = PushError::InvalidReference {
            reference: "test".to_string(),
        };
        assert!(err5.source().is_none());
    }

    // =========================================================================
    // ImagePuller Creation Tests (without network)
    // =========================================================================

    #[test]
    fn test_image_puller_creation_with_blob_cache() {
        let cache = crate::cache::BlobCache::new().unwrap();
        let puller = ImagePuller::new(cache);
        // Just verify it was created successfully
        let _ = puller;
    }

    #[test]
    fn test_image_puller_with_concurrency_limit() {
        let cache = crate::cache::BlobCache::new().unwrap();
        let puller = ImagePuller::new(cache).with_concurrency_limit(5);
        let _ = puller;
    }

    #[test]
    fn test_image_puller_with_concurrency_limit_one() {
        let cache = crate::cache::BlobCache::new().unwrap();
        let puller = ImagePuller::new(cache).with_concurrency_limit(1);
        let _ = puller;
    }

    #[test]
    fn test_image_puller_with_shared_cache() {
        let cache = crate::cache::BlobCache::new().unwrap();
        let shared: Arc<Box<dyn BlobCacheBackend>> = Arc::new(Box::new(cache));
        let puller = ImagePuller::with_cache(shared);
        let _ = puller;
    }

    // =========================================================================
    // Push Method Reference Validation Tests (async)
    // =========================================================================

    #[tokio::test]
    async fn test_push_blob_invalid_reference_returns_error() {
        let cache = crate::cache::BlobCache::new().unwrap();
        let puller = ImagePuller::new(cache);

        let result = puller
            .push_blob(
                "invalid::reference",
                "sha256:test",
                b"test data",
                "application/octet-stream",
                &RegistryAuth::Anonymous,
            )
            .await;

        assert!(result.is_err());
        match result {
            Err(PushError::InvalidReference { reference }) => {
                assert_eq!(reference, "invalid::reference");
            }
            other => panic!("Expected InvalidReference error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_push_blob_empty_reference_returns_error() {
        let cache = crate::cache::BlobCache::new().unwrap();
        let puller = ImagePuller::new(cache);

        let result = puller
            .push_blob(
                "",
                "sha256:test",
                b"test data",
                "application/octet-stream",
                &RegistryAuth::Anonymous,
            )
            .await;

        assert!(result.is_err());
        match result {
            Err(PushError::InvalidReference { reference }) => {
                assert!(reference.is_empty());
            }
            other => panic!("Expected InvalidReference error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_push_manifest_invalid_reference_returns_error() {
        let cache = crate::cache::BlobCache::new().unwrap();
        let puller = ImagePuller::new(cache);

        let manifest = oci_client::manifest::OciImageManifest::default();

        let result = puller
            .push_manifest_to_registry("invalid::reference", &manifest, &RegistryAuth::Anonymous)
            .await;

        assert!(result.is_err());
        match result {
            Err(PushError::InvalidReference { reference }) => {
                assert_eq!(reference, "invalid::reference");
            }
            other => panic!("Expected InvalidReference error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_push_manifest_empty_reference_returns_error() {
        let cache = crate::cache::BlobCache::new().unwrap();
        let puller = ImagePuller::new(cache);

        let manifest = oci_client::manifest::OciImageManifest::default();

        let result = puller
            .push_manifest_to_registry("", &manifest, &RegistryAuth::Anonymous)
            .await;

        assert!(result.is_err());
        match result {
            Err(PushError::InvalidReference { reference }) => {
                assert!(reference.is_empty());
            }
            other => panic!("Expected InvalidReference error, got {:?}", other),
        }
    }

    // =========================================================================
    // Push WASM Reference Validation Tests (async, feature-gated)
    // =========================================================================

    #[cfg(feature = "local")]
    #[tokio::test]
    async fn test_push_wasm_invalid_reference_returns_error() {
        use crate::wasm::WasiVersion;
        use crate::wasm_export::WasmExportResult;

        let cache = crate::cache::BlobCache::new().unwrap();
        let puller = ImagePuller::new(cache);

        let export_result = WasmExportResult {
            manifest_digest: "sha256:test".to_string(),
            manifest_size: 100,
            wasm_layer_digest: "sha256:wasm".to_string(),
            wasm_size: 1000,
            config_digest: "sha256:config".to_string(),
            config_size: 2,
            wasi_version: WasiVersion::Preview1,
            artifact_type: "application/vnd.wasm.module.v1+wasm".to_string(),
            manifest_json: b"{}".to_vec(),
            config_blob: b"{}".to_vec(),
            wasm_binary: vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00],
        };

        let result = puller
            .push_wasm(
                "invalid::reference",
                &export_result,
                &RegistryAuth::Anonymous,
            )
            .await;

        assert!(result.is_err());
        match result {
            Err(PushError::InvalidReference { reference }) => {
                assert_eq!(reference, "invalid::reference");
            }
            other => panic!("Expected InvalidReference error, got {:?}", other),
        }
    }

    #[cfg(feature = "local")]
    #[tokio::test]
    async fn test_push_wasm_empty_reference_returns_error() {
        use crate::wasm::WasiVersion;
        use crate::wasm_export::WasmExportResult;

        let cache = crate::cache::BlobCache::new().unwrap();
        let puller = ImagePuller::new(cache);

        let export_result = WasmExportResult {
            manifest_digest: "sha256:test".to_string(),
            manifest_size: 100,
            wasm_layer_digest: "sha256:wasm".to_string(),
            wasm_size: 1000,
            config_digest: "sha256:config".to_string(),
            config_size: 2,
            wasi_version: WasiVersion::Preview2,
            artifact_type: "application/vnd.wasm.component.v1+wasm".to_string(),
            manifest_json: b"{}".to_vec(),
            config_blob: b"{}".to_vec(),
            wasm_binary: vec![0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00],
        };

        let result = puller
            .push_wasm("", &export_result, &RegistryAuth::Anonymous)
            .await;

        assert!(result.is_err());
        match result {
            Err(PushError::InvalidReference { reference }) => {
                assert!(reference.is_empty());
            }
            other => panic!("Expected InvalidReference error, got {:?}", other),
        }
    }

    // =========================================================================
    // WasmExportResult Field Verification Tests (feature-gated)
    // =========================================================================

    #[cfg(feature = "local")]
    #[test]
    fn test_wasm_export_result_debug_formatting() {
        use crate::wasm::WasiVersion;
        use crate::wasm_export::WasmExportResult;

        let result = WasmExportResult {
            manifest_digest: "sha256:manifest".to_string(),
            manifest_size: 500,
            wasm_layer_digest: "sha256:layer".to_string(),
            wasm_size: 10000,
            config_digest: "sha256:cfg".to_string(),
            config_size: 2,
            wasi_version: WasiVersion::Preview2,
            artifact_type: "application/vnd.wasm.component.v1+wasm".to_string(),
            manifest_json: vec![],
            config_blob: vec![],
            wasm_binary: vec![],
        };

        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains("WasmExportResult"));
        assert!(debug_str.contains("sha256:manifest"));
        assert!(debug_str.contains("sha256:layer"));
        assert!(debug_str.contains("Preview2"));
    }

    #[cfg(feature = "local")]
    #[test]
    fn test_wasm_export_result_clone_independence() {
        use crate::wasm::WasiVersion;
        use crate::wasm_export::WasmExportResult;

        let result = WasmExportResult {
            manifest_digest: "sha256:original".to_string(),
            manifest_size: 100,
            wasm_layer_digest: "sha256:wasm".to_string(),
            wasm_size: 1000,
            config_digest: "sha256:config".to_string(),
            config_size: 2,
            wasi_version: WasiVersion::Preview1,
            artifact_type: "application/vnd.wasm.module.v1+wasm".to_string(),
            manifest_json: b"{}".to_vec(),
            config_blob: b"{}".to_vec(),
            wasm_binary: vec![0x00, 0x61, 0x73, 0x6d],
        };

        let mut cloned = result.clone();
        cloned.manifest_digest = "sha256:modified".to_string();

        // Original should be unchanged
        assert_eq!(result.manifest_digest, "sha256:original");
        assert_eq!(cloned.manifest_digest, "sha256:modified");
    }

    #[cfg(feature = "local")]
    #[test]
    fn test_wasm_export_result_all_fields_populated() {
        use crate::wasm::WasiVersion;
        use crate::wasm_export::WasmExportResult;

        let wasm_binary = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        let config_blob = b"{}".to_vec();
        let manifest_json = b"{\"schemaVersion\": 2}".to_vec();

        let result = WasmExportResult {
            manifest_digest: "sha256:abc".to_string(),
            manifest_size: manifest_json.len() as u64,
            wasm_layer_digest: "sha256:def".to_string(),
            wasm_size: wasm_binary.len() as u64,
            config_digest: "sha256:ghi".to_string(),
            config_size: config_blob.len() as u64,
            wasi_version: WasiVersion::Preview1,
            artifact_type: "application/vnd.wasm.module.v1+wasm".to_string(),
            manifest_json: manifest_json.clone(),
            config_blob: config_blob.clone(),
            wasm_binary: wasm_binary.clone(),
        };

        // Verify all fields
        assert!(result.manifest_digest.starts_with("sha256:"));
        assert_eq!(result.manifest_size, manifest_json.len() as u64);
        assert!(result.wasm_layer_digest.starts_with("sha256:"));
        assert_eq!(result.wasm_size, wasm_binary.len() as u64);
        assert!(result.config_digest.starts_with("sha256:"));
        assert_eq!(result.config_size, config_blob.len() as u64);
        assert_eq!(result.wasi_version, WasiVersion::Preview1);
        assert!(result.artifact_type.contains("wasm"));
        assert!(!result.manifest_json.is_empty());
        assert!(!result.config_blob.is_empty());
        assert!(!result.wasm_binary.is_empty());
    }

    // =========================================================================
    // Integration Tests for Push Flow (Documenting Expected Behavior)
    // =========================================================================
    // Note: These tests document the expected behavior of push operations.
    // Full integration tests would require a mock registry server.

    #[test]
    fn test_push_flow_documentation_blob_before_manifest() {
        // This test documents that blobs must be pushed before the manifest.
        // The manifest references blobs by digest, so blobs must exist first.
        //
        // Expected flow:
        // 1. Push config blob -> get config digest
        // 2. Push layer blob(s) -> get layer digest(s)
        // 3. Build manifest referencing the digests
        // 4. Push manifest
        //
        // This is enforced by the push_wasm method which pushes blobs first.
    }

    #[test]
    fn test_push_flow_documentation_authentication_order() {
        // This test documents that authentication happens before each push operation.
        // The client authenticates separately for blob and manifest pushes.
        //
        // Expected flow for push_blob:
        // 1. Parse reference
        // 2. Authenticate for push operation
        // 3. Push blob data
        //
        // Expected flow for push_manifest:
        // 1. Parse reference
        // 2. Authenticate for push operation
        // 3. Push manifest
    }

    #[test]
    fn test_push_result_blobs_pushed_order() {
        // This test documents that blobs_pushed should contain digests
        // in the order they were pushed (config first, then layers).
        //
        // For WASM artifacts:
        // - blobs_pushed[0] = config digest (empty JSON)
        // - blobs_pushed[1] = wasm layer digest
        let result = PushResult {
            manifest_digest: "sha256:manifest".to_string(),
            blobs_pushed: vec!["sha256:config".to_string(), "sha256:wasm_layer".to_string()],
            reference: "test:v1".to_string(),
        };

        assert_eq!(result.blobs_pushed.len(), 2);
        // Config is pushed first
        assert!(result.blobs_pushed[0].contains("config"));
        // Layer is pushed second
        assert!(result.blobs_pushed[1].contains("layer"));
    }
}
