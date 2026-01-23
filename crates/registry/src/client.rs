//! OCI distribution client for pulling images

use crate::cache::BlobCache;
use crate::error::{RegistryError, Result};
use oci_distribution::{
    client::{ClientConfig, ClientProtocol},
    secrets::RegistryAuth,
    Reference,
};
use std::sync::Arc;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::Semaphore;

/// OCI image puller with caching
pub struct ImagePuller {
    client: oci_distribution::Client,
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
        let client = oci_distribution::Client::new(config);

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
    pub async fn pull_blob(
        &self,
        image: &str,
        digest: &str,
        _auth: &RegistryAuth,
    ) -> Result<Vec<u8>> {
        // Check cache first
        if let Some(data) = self.cache.get(digest)? {
            return Ok(data);
        }

        let reference: Reference = image.parse().map_err(|_| RegistryError::NotFound {
            registry: "unknown".to_string(),
            image: image.to_string(),
        })?;

        let _permit = self.concurrency_limit.acquire().await.unwrap();

        // Pull from registry into memory
        let mut buffer = Vec::new();
        {
            let mut writer = BufWriter::new(&mut buffer);
            self.client
                .pull_blob(&reference, digest, &mut writer)
                .await
                .map_err(|_e| {
                    RegistryError::Cache(crate::error::CacheError::NotFound {
                        digest: digest.to_string(),
                    })
                })?;
            writer.flush().await.map_err(|e| {
                RegistryError::Cache(crate::error::CacheError::Database(format!(
                    "failed to flush: {}",
                    e
                )))
            })?;
        }

        // Cache the blob
        self.cache.put(digest, &buffer)?;

        Ok(buffer)
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
