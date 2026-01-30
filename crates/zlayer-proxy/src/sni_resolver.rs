//! SNI-based TLS Certificate Resolver
//!
//! This module provides dynamic TLS certificate selection based on Server Name
//! Indication (SNI). It allows serving different certificates for different domains
//! at runtime, with support for wildcard certificates.
//!
//! # Example
//!
//! ```rust,ignore
//! use zlayer_proxy::SniCertResolver;
//!
//! let resolver = SniCertResolver::new();
//!
//! // Load certificates for specific domains
//! resolver.load_cert("example.com", cert_pem, key_pem).await?;
//! resolver.load_cert("*.example.com", wildcard_cert_pem, wildcard_key_pem).await?;
//!
//! // Set a fallback certificate
//! resolver.set_default_cert(default_cert_pem, default_key_pem).await?;
//! ```

use crate::error::{ProxyError, Result};
use dashmap::DashMap;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use std::io::BufReader;
use std::sync::{Arc, RwLock};
use tracing::{debug, trace, warn};

/// SNI-based certificate resolver for dynamic TLS certificate selection
///
/// This resolver maintains a mapping of domain names to TLS certificates,
/// allowing the proxy to serve different certificates for different domains.
/// It supports:
///
/// - Exact domain matching (e.g., `api.example.com`)
/// - Wildcard certificates (e.g., `*.example.com`)
/// - A default/fallback certificate for unmatched domains
///
/// The resolver is thread-safe and supports concurrent certificate updates.
#[derive(Debug)]
pub struct SniCertResolver {
    /// Domain -> CertifiedKey mapping
    certs: DashMap<String, Arc<CertifiedKey>>,
    /// Default/fallback certificate (optional)
    default_cert: RwLock<Option<Arc<CertifiedKey>>>,
}

impl SniCertResolver {
    /// Create a new empty SNI certificate resolver
    ///
    /// # Example
    ///
    /// ```rust
    /// use zlayer_proxy::SniCertResolver;
    ///
    /// let resolver = SniCertResolver::new();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            certs: DashMap::new(),
            default_cert: RwLock::new(None),
        }
    }

    /// Load a certificate for a specific domain
    ///
    /// Parses the PEM-encoded certificate chain and private key, then stores
    /// the resulting `CertifiedKey` for the given domain.
    ///
    /// # Arguments
    ///
    /// * `domain` - The domain name (e.g., `example.com` or `*.example.com`)
    /// * `cert_pem` - PEM-encoded certificate chain
    /// * `key_pem` - PEM-encoded private key
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The certificate PEM cannot be parsed
    /// - The private key PEM cannot be parsed
    /// - The key is not compatible with the certificate
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// resolver.load_cert("example.com", cert_pem, key_pem).await?;
    /// ```
    pub async fn load_cert(&self, domain: &str, cert_pem: &str, key_pem: &str) -> Result<()> {
        let certified_key = create_certified_key(cert_pem, key_pem)?;
        let domain_normalized = normalize_domain(domain);

        debug!(domain = %domain_normalized, "Loaded TLS certificate");
        self.certs
            .insert(domain_normalized, Arc::new(certified_key));

        Ok(())
    }

    /// Set the default/fallback certificate
    ///
    /// This certificate is used when no domain-specific certificate matches
    /// the client's SNI request.
    ///
    /// # Arguments
    ///
    /// * `cert_pem` - PEM-encoded certificate chain
    /// * `key_pem` - PEM-encoded private key
    ///
    /// # Errors
    ///
    /// Returns an error if the certificate or key cannot be parsed.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// resolver.set_default_cert(default_cert_pem, default_key_pem).await?;
    /// ```
    pub async fn set_default_cert(&self, cert_pem: &str, key_pem: &str) -> Result<()> {
        let certified_key = create_certified_key(cert_pem, key_pem)?;

        debug!("Set default TLS certificate");
        let mut default = self.default_cert.write().expect("RwLock poisoned");
        *default = Some(Arc::new(certified_key));

        Ok(())
    }

    /// Remove a certificate for a specific domain
    ///
    /// # Arguments
    ///
    /// * `domain` - The domain name to remove the certificate for
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// resolver.remove_cert("example.com");
    /// ```
    pub fn remove_cert(&self, domain: &str) {
        let domain_normalized = normalize_domain(domain);
        if self.certs.remove(&domain_normalized).is_some() {
            debug!(domain = %domain_normalized, "Removed TLS certificate");
        }
    }

    /// Refresh/update a certificate for an existing domain
    ///
    /// This is equivalent to calling `load_cert` but semantically indicates
    /// an update to an existing certificate (e.g., for certificate renewal).
    ///
    /// # Arguments
    ///
    /// * `domain` - The domain name
    /// * `cert_pem` - New PEM-encoded certificate chain
    /// * `key_pem` - New PEM-encoded private key
    ///
    /// # Errors
    ///
    /// Returns an error if the certificate or key cannot be parsed.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// resolver.refresh_cert("example.com", new_cert_pem, new_key_pem).await?;
    /// ```
    pub async fn refresh_cert(&self, domain: &str, cert_pem: &str, key_pem: &str) -> Result<()> {
        let certified_key = create_certified_key(cert_pem, key_pem)?;
        let domain_normalized = normalize_domain(domain);

        debug!(domain = %domain_normalized, "Refreshed TLS certificate");
        self.certs
            .insert(domain_normalized, Arc::new(certified_key));

        Ok(())
    }

    /// Check if a certificate exists for a domain
    ///
    /// # Arguments
    ///
    /// * `domain` - The domain name to check
    ///
    /// # Returns
    ///
    /// `true` if a certificate is loaded for the exact domain name
    #[must_use]
    pub fn has_cert(&self, domain: &str) -> bool {
        let domain_normalized = normalize_domain(domain);
        self.certs.contains_key(&domain_normalized)
    }

    /// Get the number of loaded certificates
    #[must_use]
    pub fn cert_count(&self) -> usize {
        self.certs.len()
    }

    /// List all domains with loaded certificates
    #[must_use]
    pub fn domains(&self) -> Vec<String> {
        self.certs.iter().map(|r| r.key().clone()).collect()
    }

    /// Check if a default/fallback certificate is configured
    #[must_use]
    pub fn has_default_cert(&self) -> bool {
        self.default_cert
            .read()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }

    /// Internal method to resolve a certificate for a given server name
    fn resolve_cert(&self, server_name: Option<&str>) -> Option<Arc<CertifiedKey>> {
        let server_name = server_name?;
        let normalized = normalize_domain(server_name);

        // Try exact match first
        if let Some(cert) = self.certs.get(&normalized) {
            trace!(domain = %normalized, "Exact certificate match");
            return Some(Arc::clone(cert.value()));
        }

        // Try wildcard match (e.g., for "foo.example.com", try "*.example.com")
        if let Some(wildcard_domain) = get_wildcard_domain(&normalized) {
            if let Some(cert) = self.certs.get(&wildcard_domain) {
                trace!(
                    domain = %normalized,
                    wildcard = %wildcard_domain,
                    "Wildcard certificate match"
                );
                return Some(Arc::clone(cert.value()));
            }
        }

        // Fall back to default certificate
        // Note: We use std::sync::RwLock since ResolvesServerCert::resolve is sync
        if let Ok(guard) = self.default_cert.read() {
            if let Some(default) = guard.as_ref() {
                trace!(domain = %normalized, "Using default certificate");
                return Some(Arc::clone(default));
            }
        }

        warn!(domain = %normalized, "No certificate found");
        None
    }
}

impl Default for SniCertResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl ResolvesServerCert for SniCertResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let server_name = client_hello.server_name();
        self.resolve_cert(server_name)
    }
}

/// Create a `CertifiedKey` from PEM-encoded certificate and private key
///
/// # Arguments
///
/// * `cert_pem` - PEM-encoded certificate chain (may contain multiple certificates)
/// * `key_pem` - PEM-encoded private key (PKCS#1, PKCS#8, or SEC1 format)
///
/// # Errors
///
/// Returns an error if:
/// - The certificate PEM cannot be parsed
/// - No certificates are found in the PEM
/// - The private key PEM cannot be parsed
/// - No private key is found in the PEM
/// - The key cannot be converted to a signing key
fn create_certified_key(cert_pem: &str, key_pem: &str) -> Result<CertifiedKey> {
    // Parse certificates
    let certs = parse_certificates(cert_pem)?;
    if certs.is_empty() {
        return Err(ProxyError::Tls("No certificates found in PEM".to_string()));
    }

    // Parse private key
    let key = parse_private_key(key_pem)?;

    // Create signing key using rustls crypto provider
    let signing_key = rustls::crypto::ring::sign::any_supported_type(&key)
        .map_err(|e| ProxyError::Tls(format!("Failed to create signing key: {e}")))?;

    Ok(CertifiedKey::new(certs, signing_key))
}

/// Parse PEM-encoded certificates
fn parse_certificates(pem: &str) -> Result<Vec<CertificateDer<'static>>> {
    let mut reader = BufReader::new(pem.as_bytes());
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| ProxyError::Tls(format!("Failed to parse certificate PEM: {e}")))?;

    Ok(certs)
}

/// Parse a PEM-encoded private key
fn parse_private_key(pem: &str) -> Result<PrivateKeyDer<'static>> {
    let mut reader = BufReader::new(pem.as_bytes());

    // Try to read different key formats
    loop {
        match rustls_pemfile::read_one(&mut reader) {
            Ok(Some(rustls_pemfile::Item::Pkcs1Key(key))) => {
                return Ok(PrivateKeyDer::Pkcs1(key));
            }
            Ok(Some(rustls_pemfile::Item::Pkcs8Key(key))) => {
                return Ok(PrivateKeyDer::Pkcs8(key));
            }
            Ok(Some(rustls_pemfile::Item::Sec1Key(key))) => {
                return Ok(PrivateKeyDer::Sec1(key));
            }
            Ok(Some(_)) => {
                // Skip non-key items (like certificates)
                continue;
            }
            Ok(None) => {
                return Err(ProxyError::Tls("No private key found in PEM".to_string()));
            }
            Err(e) => {
                return Err(ProxyError::Tls(format!(
                    "Failed to parse private key PEM: {e}"
                )));
            }
        }
    }
}

/// Normalize a domain name (lowercase, trim whitespace)
fn normalize_domain(domain: &str) -> String {
    domain.trim().to_lowercase()
}

/// Get the wildcard domain for a given domain
///
/// For `foo.example.com`, returns `*.example.com`.
/// For `example.com` (no subdomain), returns `None`.
fn get_wildcard_domain(domain: &str) -> Option<String> {
    let parts: Vec<&str> = domain.split('.').collect();
    if parts.len() > 2 {
        // Has subdomain, create wildcard
        Some(format!("*.{}", parts[1..].join(".")))
    } else {
        // No subdomain (e.g., "example.com")
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_domain() {
        assert_eq!(normalize_domain("Example.COM"), "example.com");
        assert_eq!(normalize_domain("  foo.bar.com  "), "foo.bar.com");
        assert_eq!(normalize_domain("API.Example.ORG"), "api.example.org");
    }

    #[test]
    fn test_get_wildcard_domain() {
        assert_eq!(
            get_wildcard_domain("foo.example.com"),
            Some("*.example.com".to_string())
        );
        assert_eq!(
            get_wildcard_domain("bar.foo.example.com"),
            Some("*.foo.example.com".to_string())
        );
        assert_eq!(get_wildcard_domain("example.com"), None);
        assert_eq!(get_wildcard_domain("localhost"), None);
    }

    #[test]
    fn test_sni_resolver_new() {
        let resolver = SniCertResolver::new();
        assert_eq!(resolver.cert_count(), 0);
        assert!(resolver.domains().is_empty());
    }

    #[test]
    fn test_sni_resolver_default() {
        let resolver = SniCertResolver::default();
        assert_eq!(resolver.cert_count(), 0);
    }

    // Generate a self-signed certificate for testing
    fn generate_test_cert() -> (String, String) {
        use rcgen::{generate_simple_self_signed, CertifiedKey as RcgenCertifiedKey};

        let subject_alt_names = vec!["localhost".to_string(), "example.com".to_string()];
        let RcgenCertifiedKey { cert, key_pair } =
            generate_simple_self_signed(subject_alt_names).unwrap();

        (cert.pem(), key_pair.serialize_pem())
    }

    #[tokio::test]
    async fn test_load_cert() {
        let resolver = SniCertResolver::new();
        let (cert_pem, key_pem) = generate_test_cert();

        let result = resolver.load_cert("example.com", &cert_pem, &key_pem).await;
        assert!(result.is_ok());
        assert!(resolver.has_cert("example.com"));
        assert_eq!(resolver.cert_count(), 1);
    }

    #[tokio::test]
    async fn test_load_cert_case_insensitive() {
        let resolver = SniCertResolver::new();
        let (cert_pem, key_pem) = generate_test_cert();

        resolver
            .load_cert("Example.COM", &cert_pem, &key_pem)
            .await
            .unwrap();
        assert!(resolver.has_cert("example.com"));
        assert!(resolver.has_cert("EXAMPLE.COM"));
    }

    #[tokio::test]
    async fn test_remove_cert() {
        let resolver = SniCertResolver::new();
        let (cert_pem, key_pem) = generate_test_cert();

        resolver
            .load_cert("example.com", &cert_pem, &key_pem)
            .await
            .unwrap();
        assert!(resolver.has_cert("example.com"));

        resolver.remove_cert("example.com");
        assert!(!resolver.has_cert("example.com"));
        assert_eq!(resolver.cert_count(), 0);
    }

    #[tokio::test]
    async fn test_refresh_cert() {
        let resolver = SniCertResolver::new();
        let (cert_pem, key_pem) = generate_test_cert();

        // Load initial cert
        resolver
            .load_cert("example.com", &cert_pem, &key_pem)
            .await
            .unwrap();

        // Refresh with new cert
        let (new_cert_pem, new_key_pem) = generate_test_cert();
        let result = resolver
            .refresh_cert("example.com", &new_cert_pem, &new_key_pem)
            .await;
        assert!(result.is_ok());
        assert_eq!(resolver.cert_count(), 1);
    }

    #[tokio::test]
    async fn test_set_default_cert() {
        let resolver = SniCertResolver::new();
        let (cert_pem, key_pem) = generate_test_cert();

        let result = resolver.set_default_cert(&cert_pem, &key_pem).await;
        assert!(result.is_ok());

        // Default cert should not show up in cert_count or domains
        assert_eq!(resolver.cert_count(), 0);
    }

    #[tokio::test]
    async fn test_has_default_cert() {
        let resolver = SniCertResolver::new();
        assert!(!resolver.has_default_cert());

        let (cert_pem, key_pem) = generate_test_cert();
        resolver
            .set_default_cert(&cert_pem, &key_pem)
            .await
            .unwrap();

        assert!(resolver.has_default_cert());
    }

    #[tokio::test]
    async fn test_domains() {
        let resolver = SniCertResolver::new();
        let (cert_pem, key_pem) = generate_test_cert();

        resolver
            .load_cert("api.example.com", &cert_pem, &key_pem)
            .await
            .unwrap();
        resolver
            .load_cert("web.example.com", &cert_pem, &key_pem)
            .await
            .unwrap();

        let domains = resolver.domains();
        assert_eq!(domains.len(), 2);
        assert!(domains.contains(&"api.example.com".to_string()));
        assert!(domains.contains(&"web.example.com".to_string()));
    }

    #[tokio::test]
    async fn test_resolve_exact_match() {
        let resolver = SniCertResolver::new();
        let (cert_pem, key_pem) = generate_test_cert();

        resolver
            .load_cert("example.com", &cert_pem, &key_pem)
            .await
            .unwrap();

        let result = resolver.resolve_cert(Some("example.com"));
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn test_resolve_wildcard_match() {
        let resolver = SniCertResolver::new();
        let (cert_pem, key_pem) = generate_test_cert();

        // Load wildcard cert
        resolver
            .load_cert("*.example.com", &cert_pem, &key_pem)
            .await
            .unwrap();

        // Should match subdomains
        let result = resolver.resolve_cert(Some("api.example.com"));
        assert!(result.is_some());

        let result = resolver.resolve_cert(Some("web.example.com"));
        assert!(result.is_some());

        // Should not match base domain
        let result = resolver.resolve_cert(Some("example.com"));
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_resolve_default_fallback() {
        let resolver = SniCertResolver::new();
        let (cert_pem, key_pem) = generate_test_cert();

        resolver
            .set_default_cert(&cert_pem, &key_pem)
            .await
            .unwrap();

        // Unknown domain should fall back to default
        let result = resolver.resolve_cert(Some("unknown.com"));
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn test_resolve_no_match() {
        let resolver = SniCertResolver::new();
        let (cert_pem, key_pem) = generate_test_cert();

        resolver
            .load_cert("example.com", &cert_pem, &key_pem)
            .await
            .unwrap();

        // No default, different domain
        let result = resolver.resolve_cert(Some("other.com"));
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_resolve_none_server_name() {
        let resolver = SniCertResolver::new();

        // No server name provided
        let result = resolver.resolve_cert(None);
        assert!(result.is_none());
    }

    #[test]
    fn test_invalid_cert_pem() {
        let result = parse_certificates("not a valid PEM");
        assert!(result.is_ok()); // Will succeed but return empty vec
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_invalid_key_pem() {
        let result = parse_private_key("not a valid PEM");
        assert!(result.is_err());
    }

    #[test]
    fn test_create_certified_key_empty_certs() {
        let (_, key_pem) = generate_test_cert();
        let result = create_certified_key("", &key_pem);
        assert!(result.is_err());
    }
}
