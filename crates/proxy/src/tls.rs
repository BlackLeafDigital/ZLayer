//! TLS server configuration
//!
//! This module provides TLS termination capabilities for the proxy server.

use crate::config::{TlsConfig, TlsVersion};
use crate::error::{ProxyError, Result};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::ServerConfig;
use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;
use tokio_rustls::TlsAcceptor;
use tracing::{debug, info};

/// TLS server configuration builder
#[derive(Debug, Clone)]
pub struct TlsServerConfig {
    /// Path to certificate file (PEM format)
    pub cert_path: String,
    /// Path to private key file (PEM format)
    pub key_path: String,
    /// ALPN protocols to advertise
    pub alpn_protocols: Vec<Vec<u8>>,
    /// Minimum TLS version
    pub min_version: TlsVersion,
}

impl TlsServerConfig {
    /// Create a new TLS server configuration
    pub fn new(cert_path: impl Into<String>, key_path: impl Into<String>) -> Self {
        Self {
            cert_path: cert_path.into(),
            key_path: key_path.into(),
            alpn_protocols: vec![b"h2".to_vec(), b"http/1.1".to_vec()],
            min_version: TlsVersion::Tls12,
        }
    }

    /// Set ALPN protocols
    pub fn with_alpn_protocols(mut self, protocols: Vec<Vec<u8>>) -> Self {
        self.alpn_protocols = protocols;
        self
    }

    /// Set minimum TLS version
    pub fn with_min_version(mut self, version: TlsVersion) -> Self {
        self.min_version = version;
        self
    }

    /// Create from a TlsConfig
    pub fn from_config(config: &TlsConfig) -> Self {
        let mut alpn = vec![];
        if config.alpn_h2 {
            alpn.push(b"h2".to_vec());
        }
        alpn.push(b"http/1.1".to_vec());

        Self {
            cert_path: config.cert_path.to_string_lossy().to_string(),
            key_path: config.key_path.to_string_lossy().to_string(),
            alpn_protocols: alpn,
            min_version: config.min_version,
        }
    }
}

/// Load certificates from a PEM file
pub fn load_certs(path: &str) -> Result<Vec<CertificateDer<'static>>> {
    let file = File::open(path).map_err(|e| {
        ProxyError::Tls(format!("Failed to open certificate file '{}': {}", path, e))
    })?;

    let mut reader = BufReader::new(file);
    let certs: Vec<_> = rustls_pemfile::certs(&mut reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| ProxyError::Tls(format!("Failed to parse certificates from '{}': {}", path, e)))?;

    if certs.is_empty() {
        return Err(ProxyError::Tls(format!(
            "No certificates found in '{}'",
            path
        )));
    }

    debug!(count = certs.len(), path = %path, "Loaded certificates");
    Ok(certs)
}

/// Load a private key from a PEM file
pub fn load_private_key(path: &str) -> Result<PrivateKeyDer<'static>> {
    let file = File::open(path).map_err(|e| {
        ProxyError::Tls(format!("Failed to open private key file '{}': {}", path, e))
    })?;

    let mut reader = BufReader::new(file);

    // Try to read different key formats
    loop {
        match rustls_pemfile::read_one(&mut reader) {
            Ok(Some(rustls_pemfile::Item::Pkcs1Key(key))) => {
                debug!(path = %path, "Loaded PKCS#1 RSA private key");
                return Ok(PrivateKeyDer::Pkcs1(key));
            }
            Ok(Some(rustls_pemfile::Item::Pkcs8Key(key))) => {
                debug!(path = %path, "Loaded PKCS#8 private key");
                return Ok(PrivateKeyDer::Pkcs8(key));
            }
            Ok(Some(rustls_pemfile::Item::Sec1Key(key))) => {
                debug!(path = %path, "Loaded SEC1 EC private key");
                return Ok(PrivateKeyDer::Sec1(key));
            }
            Ok(Some(_)) => {
                // Skip non-key items (like certificates)
                continue;
            }
            Ok(None) => {
                return Err(ProxyError::Tls(format!(
                    "No private key found in '{}'",
                    path
                )));
            }
            Err(e) => {
                return Err(ProxyError::Tls(format!(
                    "Failed to parse private key from '{}': {}",
                    path, e
                )));
            }
        }
    }
}

/// Create a TLS acceptor from configuration
pub fn create_tls_acceptor(config: &TlsServerConfig) -> Result<TlsAcceptor> {
    let certs = load_certs(&config.cert_path)?;
    let key = load_private_key(&config.key_path)?;

    let server_config = create_server_config(certs, key, config)?;

    info!(
        cert_path = %config.cert_path,
        min_version = ?config.min_version,
        alpn = ?config.alpn_protocols.iter()
            .map(|p| String::from_utf8_lossy(p).to_string())
            .collect::<Vec<_>>(),
        "Created TLS acceptor"
    );

    Ok(TlsAcceptor::from(Arc::new(server_config)))
}

/// Create a rustls ServerConfig
fn create_server_config(
    certs: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
    config: &TlsServerConfig,
) -> Result<ServerConfig> {
    // Select TLS protocol versions based on minimum version
    let versions: Vec<&'static rustls::SupportedProtocolVersion> = match config.min_version {
        TlsVersion::Tls12 => vec![&rustls::version::TLS13, &rustls::version::TLS12],
        TlsVersion::Tls13 => vec![&rustls::version::TLS13],
    };

    let mut server_config = ServerConfig::builder_with_protocol_versions(&versions)
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| ProxyError::Tls(format!("Failed to create TLS config: {}", e)))?;

    // Configure ALPN
    if !config.alpn_protocols.is_empty() {
        server_config.alpn_protocols = config.alpn_protocols.clone();
    }

    Ok(server_config)
}

/// Create a TLS acceptor directly from file paths
pub fn create_acceptor_from_files(
    cert_path: &str,
    key_path: &str,
) -> Result<TlsAcceptor> {
    let config = TlsServerConfig::new(cert_path, key_path);
    create_tls_acceptor(&config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tls_server_config_creation() {
        let config = TlsServerConfig::new("/path/to/cert.pem", "/path/to/key.pem");
        assert_eq!(config.cert_path, "/path/to/cert.pem");
        assert_eq!(config.key_path, "/path/to/key.pem");
        assert_eq!(config.alpn_protocols, vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
        assert_eq!(config.min_version, TlsVersion::Tls12);
    }

    #[test]
    fn test_tls_server_config_builder() {
        let config = TlsServerConfig::new("/path/to/cert.pem", "/path/to/key.pem")
            .with_alpn_protocols(vec![b"http/1.1".to_vec()])
            .with_min_version(TlsVersion::Tls13);

        assert_eq!(config.alpn_protocols, vec![b"http/1.1".to_vec()]);
        assert_eq!(config.min_version, TlsVersion::Tls13);
    }

    #[test]
    fn test_load_certs_file_not_found() {
        let result = load_certs("/nonexistent/path/cert.pem");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ProxyError::Tls(_)));
    }

    #[test]
    fn test_load_private_key_file_not_found() {
        let result = load_private_key("/nonexistent/path/key.pem");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ProxyError::Tls(_)));
    }

    #[test]
    fn test_from_tls_config() {
        let tls_config = TlsConfig {
            cert_path: "/path/to/cert.pem".into(),
            key_path: "/path/to/key.pem".into(),
            min_version: TlsVersion::Tls13,
            alpn_h2: true,
        };

        let server_config = TlsServerConfig::from_config(&tls_config);
        assert_eq!(server_config.cert_path, "/path/to/cert.pem");
        assert_eq!(server_config.key_path, "/path/to/key.pem");
        assert_eq!(server_config.min_version, TlsVersion::Tls13);
        assert!(server_config.alpn_protocols.contains(&b"h2".to_vec()));
    }

    #[test]
    fn test_from_tls_config_no_h2() {
        let tls_config = TlsConfig {
            cert_path: "/path/to/cert.pem".into(),
            key_path: "/path/to/key.pem".into(),
            min_version: TlsVersion::Tls12,
            alpn_h2: false,
        };

        let server_config = TlsServerConfig::from_config(&tls_config);
        assert!(!server_config.alpn_protocols.contains(&b"h2".to_vec()));
        assert!(server_config.alpn_protocols.contains(&b"http/1.1".to_vec()));
    }
}
