//! ACME certificate manager for automatic TLS
//!
//! This module provides automatic TLS certificate provisioning and management
//! using the ACME protocol (Let's Encrypt compatible).
//!
//! NOTE: This is a minimal stub implementation for Phase 2 (ZLayerProxy).
//! Full ACME implementation will be completed in Phase 4.

use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::RwLock;

/// Certificate manager for TLS certificate provisioning and caching
///
/// The CertManager handles:
/// - Loading existing certificates from disk
/// - Caching certificates in memory
/// - Provisioning new certificates via ACME (future)
pub struct CertManager {
    /// Path to certificate storage directory
    storage_path: PathBuf,
    /// Optional ACME account email
    acme_email: Option<String>,
    /// Certificate cache (domain -> (cert_pem, key_pem))
    cache: RwLock<HashMap<String, (String, String)>>,
}

impl CertManager {
    /// Create a new certificate manager
    ///
    /// # Arguments
    /// * `storage_path` - Directory to store certificates
    /// * `acme_email` - Optional email for ACME account registration
    pub async fn new(
        storage_path: String,
        acme_email: Option<String>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let storage_path = PathBuf::from(storage_path);

        // Create storage directory if it doesn't exist
        if !storage_path.exists() {
            tokio::fs::create_dir_all(&storage_path).await?;
        }

        Ok(Self {
            storage_path,
            acme_email,
            cache: RwLock::new(HashMap::new()),
        })
    }

    /// Get a certificate for a domain
    ///
    /// This method:
    /// 1. Checks the memory cache
    /// 2. Checks disk storage
    /// 3. Provisions via ACME if not found (future)
    ///
    /// # Arguments
    /// * `domain` - The domain to get a certificate for
    ///
    /// # Returns
    /// Tuple of (certificate_pem, private_key_pem)
    pub async fn get_cert(
        &self,
        domain: &str,
    ) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
        // Check memory cache
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(domain) {
                return Ok(cached.clone());
            }
        }

        // Check disk storage
        let cert_path = self.storage_path.join(format!("{}.crt", domain));
        let key_path = self.storage_path.join(format!("{}.key", domain));

        if cert_path.exists() && key_path.exists() {
            let cert = tokio::fs::read_to_string(&cert_path).await?;
            let key = tokio::fs::read_to_string(&key_path).await?;

            // Cache for future use
            {
                let mut cache = self.cache.write().await;
                cache.insert(domain.to_string(), (cert.clone(), key.clone()));
            }

            return Ok((cert, key));
        }

        // Provision via ACME (not yet implemented)
        self.provision_cert(domain).await
    }

    /// Store a certificate
    ///
    /// # Arguments
    /// * `domain` - The domain
    /// * `cert` - Certificate PEM content
    /// * `key` - Private key PEM content
    pub async fn store_cert(
        &self,
        domain: &str,
        cert: &str,
        key: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let cert_path = self.storage_path.join(format!("{}.crt", domain));
        let key_path = self.storage_path.join(format!("{}.key", domain));

        // Write to disk
        tokio::fs::write(&cert_path, cert).await?;
        tokio::fs::write(&key_path, key).await?;

        // Update cache
        {
            let mut cache = self.cache.write().await;
            cache.insert(domain.to_string(), (cert.to_string(), key.to_string()));
        }

        tracing::info!(domain = %domain, "Stored certificate");
        Ok(())
    }

    /// Check if a certificate exists for a domain
    pub async fn has_cert(&self, domain: &str) -> bool {
        // Check cache first
        {
            let cache = self.cache.read().await;
            if cache.contains_key(domain) {
                return true;
            }
        }

        // Check disk
        let cert_path = self.storage_path.join(format!("{}.crt", domain));
        let key_path = self.storage_path.join(format!("{}.key", domain));

        cert_path.exists() && key_path.exists()
    }

    /// Get the ACME email (if configured)
    pub fn acme_email(&self) -> Option<&str> {
        self.acme_email.as_deref()
    }

    /// Get the storage path
    pub fn storage_path(&self) -> &PathBuf {
        &self.storage_path
    }

    /// Provision a certificate via ACME
    ///
    /// NOTE: This is a stub that will be implemented in Phase 4.
    /// For now, it returns an error indicating ACME is not yet available.
    async fn provision_cert(
        &self,
        domain: &str,
    ) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
        tracing::warn!(
            domain = %domain,
            "ACME certificate provisioning not yet implemented"
        );

        Err(format!(
            "Certificate for '{}' not found and ACME provisioning is not yet implemented. \
             Please manually provide certificates at {} or wait for Phase 4 implementation.",
            domain,
            self.storage_path.display()
        )
        .into())
    }

    /// Clear the certificate cache
    pub async fn clear_cache(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
    }

    /// Get cached certificate count
    pub async fn cached_count(&self) -> usize {
        let cache = self.cache.read().await;
        cache.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_cert_manager_creation() {
        let dir = tempdir().unwrap();
        let manager = CertManager::new(
            dir.path().to_string_lossy().to_string(),
            Some("test@example.com".to_string()),
        )
        .await
        .unwrap();

        assert_eq!(manager.acme_email(), Some("test@example.com"));
        assert!(manager.storage_path().exists());
    }

    #[tokio::test]
    async fn test_store_and_get_cert() {
        let dir = tempdir().unwrap();
        let manager = CertManager::new(dir.path().to_string_lossy().to_string(), None)
            .await
            .unwrap();

        // Store a test certificate
        let cert = "-----BEGIN CERTIFICATE-----\ntest\n-----END CERTIFICATE-----";
        let key = "-----BEGIN PRIVATE KEY-----\ntest\n-----END PRIVATE KEY-----";

        manager
            .store_cert("test.example.com", cert, key)
            .await
            .unwrap();

        // Should be in cache
        assert!(manager.has_cert("test.example.com").await);

        // Clear cache and verify disk read
        manager.clear_cache().await;
        assert!(manager.has_cert("test.example.com").await);

        // Get the certificate
        let (retrieved_cert, retrieved_key) = manager.get_cert("test.example.com").await.unwrap();
        assert_eq!(retrieved_cert, cert);
        assert_eq!(retrieved_key, key);
    }

    #[tokio::test]
    async fn test_cert_not_found() {
        let dir = tempdir().unwrap();
        let manager = CertManager::new(dir.path().to_string_lossy().to_string(), None)
            .await
            .unwrap();

        let result = manager.get_cert("nonexistent.example.com").await;
        assert!(result.is_err());
    }
}
