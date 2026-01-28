//! OCI distribution client for pulling images

use crate::cache::BlobCacheBackend;
use crate::error::{RegistryError, Result};
use crate::wasm::{detect_artifact_type, extract_wasm_info, ArtifactType, WasmArtifactInfo};
use oci_client::{
    client::{ClientConfig, ClientProtocol},
    manifest::OciImageManifest,
    secrets::RegistryAuth,
    Reference,
};
use std::sync::Arc;
use tokio::sync::Semaphore;

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
                format!("blob {} was empty after pulling from registry", digest),
            )));
        }

        // Cache the blob (now async)
        self.cache.put(digest, &buffer).await?;

        tracing::debug!(digest = %digest, size = buffer.len(), "blob cached successfully");

        Ok(buffer)
    }

    /// Pull an image manifest from the registry
    ///
    /// Returns the manifest and its digest.
    pub async fn pull_manifest(
        &self,
        image: &str,
        auth: &RegistryAuth,
    ) -> Result<(OciImageManifest, String)> {
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
            image: format!("{} (not a WASM artifact)", image),
        })?;

        let wasm_digest =
            wasm_info
                .wasm_layer_digest
                .as_ref()
                .ok_or_else(|| RegistryError::NotFound {
                    registry: "unknown".to_string(),
                    image: format!("{} (no WASM layer found)", image),
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
                    format!("layer {} ({}) is empty after pull", i, layer.digest),
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

    #[test]
    fn test_image_display() {
        let image = Image {
            registry: "docker.io".to_string(),
            repository: "library/nginx".to_string(),
            tag: "latest".to_string(),
        };
        assert_eq!(image.to_string(), "docker.io/library/nginx:latest");
    }
}
