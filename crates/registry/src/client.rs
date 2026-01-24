//! OCI distribution client for pulling images

use crate::cache::BlobCache;
use crate::error::{RegistryError, Result};
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
    cache: Arc<BlobCache>,
    concurrency_limit: Arc<Semaphore>,
}

impl ImagePuller {
    /// Create a new image puller
    pub fn new(cache: Arc<BlobCache>) -> Self {
        let config = ClientConfig {
            protocol: ClientProtocol::Https,
            ..Default::default()
        };
        let client = oci_client::Client::new(config);

        Self {
            client,
            cache,
            concurrency_limit: Arc::new(Semaphore::new(3)),
        }
    }

    /// Set concurrency limit for blob downloads
    pub fn with_concurrency_limit(mut self, limit: usize) -> Self {
        self.concurrency_limit = Arc::new(Semaphore::new(limit));
        self
    }

    /// Pull a single blob and cache it
    ///
    /// Note: Authentication is handled internally by the oci_client.
    /// The `auth` parameter is reserved for future use with explicit auth flows.
    pub async fn pull_blob(
        &self,
        image: &str,
        digest: &str,
        _auth: &RegistryAuth,
    ) -> Result<Vec<u8>> {
        // Check cache first
        if let Some(data) = self.cache.get(digest)? {
            tracing::debug!(digest = %digest, "blob found in cache");
            return Ok(data);
        }

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

        // Cache the blob
        self.cache.put(digest, &buffer)?;

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
