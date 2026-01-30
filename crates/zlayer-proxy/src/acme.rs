//! ACME certificate manager for automatic TLS
//!
//! This module provides automatic TLS certificate provisioning and management
//! using the ACME protocol (Let's Encrypt compatible).
//!
//! Implements the full ACME protocol using instant-acme for HTTP-01 challenges.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, TimeDelta, Utc};
use dashmap::DashMap;
use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, ChallengeType, Identifier, NewAccount,
    NewOrder, OrderStatus,
};
use rcgen::{CertificateParams, KeyPair};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use x509_parser::pem::parse_x509_pem;

use crate::sni_resolver::SniCertResolver;

/// Challenge expiration time (5 minutes)
const CHALLENGE_EXPIRATION: Duration = Duration::from_secs(5 * 60);

/// Default renewal threshold (30 days before expiry)
const RENEWAL_THRESHOLD_DAYS: i64 = 30;

/// Maximum time to wait for ACME validation (120 seconds)
const ACME_VALIDATION_TIMEOUT: Duration = Duration::from_secs(120);

/// Default ACME directory URL (Let's Encrypt production)
pub const LETS_ENCRYPT_PRODUCTION: &str = "https://acme-v02.api.letsencrypt.org/directory";

/// Let's Encrypt staging directory URL (for testing)
pub const LETS_ENCRYPT_STAGING: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";

/// ACME HTTP-01 challenge token for domain validation
#[derive(Debug, Clone)]
pub struct ChallengeToken {
    /// The challenge token from ACME server
    pub token: String,
    /// The key authorization response (token.thumbprint)
    pub key_authorization: String,
    /// The domain being validated
    pub domain: String,
    /// When this challenge was created
    pub created_at: Instant,
}

/// ACME account credentials for persistent account management
///
/// This struct stores the account information needed to authenticate
/// with an ACME server (e.g., Let's Encrypt) across restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcmeAccount {
    /// The account URL returned by the ACME server after registration
    pub account_url: String,
    /// P-256 ECDSA private key in PEM format
    pub account_key_pem: String,
    /// Contact email addresses for the account
    pub contact: Vec<String>,
    /// When the account was created
    pub created_at: DateTime<Utc>,
}

/// Metadata about a stored certificate for tracking expiry and renewal
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertMetadata {
    /// The domain this certificate is for
    pub domain: String,
    /// Certificate validity start time
    pub not_before: DateTime<Utc>,
    /// Certificate expiry time
    pub not_after: DateTime<Utc>,
    /// When this certificate was provisioned/stored
    pub provisioned_at: DateTime<Utc>,
    /// SHA256 fingerprint of the certificate
    pub fingerprint: String,
}

/// Certificate manager for TLS certificate provisioning and caching
///
/// The CertManager handles:
/// - Loading existing certificates from disk
/// - Caching certificates in memory
/// - Provisioning new certificates via ACME
pub struct CertManager {
    /// Path to certificate storage directory
    storage_path: PathBuf,
    /// Optional ACME account email
    acme_email: Option<String>,
    /// ACME directory URL (e.g., Let's Encrypt)
    acme_directory: String,
    /// Certificate cache (domain -> (cert_pem, key_pem))
    cache: RwLock<HashMap<String, (String, String)>>,
    /// ACME HTTP-01 challenge tokens (token -> ChallengeToken)
    challenges: DashMap<String, ChallengeToken>,
    /// Cached ACME account metadata (for display/persistence)
    account: RwLock<Option<AcmeAccount>>,
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
        Self::with_directory(
            storage_path,
            acme_email,
            LETS_ENCRYPT_PRODUCTION.to_string(),
        )
        .await
    }

    /// Create a new certificate manager with a custom ACME directory
    ///
    /// # Arguments
    /// * `storage_path` - Directory to store certificates
    /// * `acme_email` - Optional email for ACME account registration
    /// * `acme_directory` - ACME directory URL (e.g., Let's Encrypt production/staging)
    pub async fn with_directory(
        storage_path: String,
        acme_email: Option<String>,
        acme_directory: String,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let storage_path = PathBuf::from(storage_path);

        // Create storage directory if it doesn't exist
        if !storage_path.exists() {
            tokio::fs::create_dir_all(&storage_path).await?;
        }

        let manager = Self {
            storage_path,
            acme_email,
            acme_directory,
            cache: RwLock::new(HashMap::new()),
            challenges: DashMap::new(),
            account: RwLock::new(None),
        };

        // Try to load existing account from disk
        if let Some(account) = manager.load_account().await {
            tracing::info!(
                account_url = %account.account_url,
                "Loaded existing ACME account from disk"
            );
            *manager.account.write().await = Some(account);
        }

        Ok(manager)
    }

    /// Get the ACME directory URL
    pub fn acme_directory(&self) -> &str {
        &self.acme_directory
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
    /// This method stores the certificate and key to disk, updates the memory cache,
    /// and extracts/saves certificate metadata for renewal tracking.
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

        // Extract and save certificate metadata for renewal tracking
        if let Ok((not_before, not_after)) = Self::parse_cert_expiry(cert) {
            let fingerprint = Self::compute_cert_fingerprint(cert);
            let metadata = CertMetadata {
                domain: domain.to_string(),
                not_before,
                not_after,
                provisioned_at: Utc::now(),
                fingerprint,
            };
            if let Err(e) = self.save_cert_metadata(&metadata).await {
                tracing::warn!(
                    domain = %domain,
                    error = %e,
                    "Failed to save certificate metadata"
                );
            }
        } else {
            tracing::debug!(
                domain = %domain,
                "Could not parse certificate expiry dates for metadata"
            );
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
    /// This method implements the full ACME protocol using HTTP-01 challenges:
    /// 1. Gets or creates an ACME account
    /// 2. Creates a new order for the domain
    /// 3. Processes HTTP-01 challenges
    /// 4. Waits for validation
    /// 5. Generates a keypair and CSR
    /// 6. Finalizes the order
    /// 7. Retrieves and stores the certificate
    ///
    /// # Arguments
    /// * `domain` - The domain to provision a certificate for
    ///
    /// # Returns
    /// Tuple of (certificate_pem, private_key_pem)
    async fn provision_cert(
        &self,
        domain: &str,
    ) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
        tracing::info!(domain = %domain, "Starting ACME certificate provisioning");

        // Require an email for ACME registration
        if self.acme_email.is_none() {
            return Err(format!(
                "ACME email is required for certificate provisioning. \
                 Certificate for '{}' not found. Please configure an ACME email \
                 or manually provide certificates at {}",
                domain,
                self.storage_path.display()
            )
            .into());
        }

        // Step 1: Get or create ACME account
        let account = self.get_or_create_acme_account().await?;

        // Step 2: Create new order
        let identifiers = [Identifier::Dns(domain.to_string())];
        let new_order = NewOrder {
            identifiers: &identifiers,
        };
        let mut order = account.new_order(&new_order).await.map_err(|e| {
            tracing::error!(domain = %domain, error = %e, "Failed to create ACME order");
            format!("Failed to create ACME order for '{}': {}", domain, e)
        })?;

        tracing::debug!(domain = %domain, "Created ACME order");

        // Step 3: Get authorizations and process HTTP-01 challenges
        let authorizations = order.authorizations().await.map_err(|e| {
            tracing::error!(domain = %domain, error = %e, "Failed to get authorizations");
            format!("Failed to get authorizations for '{}': {}", domain, e)
        })?;

        for auth in authorizations {
            tracing::debug!(
                domain = %domain,
                status = ?auth.status,
                "Processing authorization"
            );

            // Skip already valid authorizations
            if auth.status == AuthorizationStatus::Valid {
                continue;
            }

            // Find HTTP-01 challenge
            let challenge = auth
                .challenges
                .iter()
                .find(|c| c.r#type == ChallengeType::Http01)
                .ok_or_else(|| {
                    format!(
                        "No HTTP-01 challenge available for '{}'. Available types: {:?}",
                        domain,
                        auth.challenges
                            .iter()
                            .map(|c| &c.r#type)
                            .collect::<Vec<_>>()
                    )
                })?;

            // Store challenge for our HTTP server to serve
            let key_auth = order.key_authorization(challenge);
            self.store_challenge(&challenge.token, domain, key_auth.as_str())
                .await;

            tracing::info!(
                domain = %domain,
                token = %challenge.token,
                "Stored HTTP-01 challenge, notifying ACME server"
            );

            // Tell ACME server we're ready
            order
                .set_challenge_ready(&challenge.url)
                .await
                .map_err(|e| {
                    tracing::error!(
                        domain = %domain,
                        error = %e,
                        "Failed to set challenge ready"
                    );
                    format!("Failed to set challenge ready for '{}': {}", domain, e)
                })?;
        }

        // Step 4: Wait for order to be ready (with timeout)
        let start_time = Instant::now();
        loop {
            if start_time.elapsed() > ACME_VALIDATION_TIMEOUT {
                self.clear_challenges_for_domain(domain).await;
                return Err(format!(
                    "ACME validation timeout for '{}'. Validation did not complete within {} seconds. \
                     Ensure the domain is accessible at http://{}/.well-known/acme-challenge/",
                    domain,
                    ACME_VALIDATION_TIMEOUT.as_secs(),
                    domain
                )
                .into());
            }

            order.refresh().await.map_err(|e| {
                tracing::error!(domain = %domain, error = %e, "Failed to refresh order status");
                format!("Failed to refresh order status for '{}': {}", domain, e)
            })?;

            let status = order.state().status;
            tracing::debug!(domain = %domain, status = ?status, "Order status");

            match status {
                OrderStatus::Ready => {
                    tracing::info!(domain = %domain, "Order is ready for finalization");
                    break;
                }
                OrderStatus::Invalid => {
                    self.clear_challenges_for_domain(domain).await;
                    let error_msg = order
                        .state()
                        .error
                        .as_ref()
                        .map(|e| format!("{:?}", e))
                        .unwrap_or_else(|| "Unknown error".to_string());
                    return Err(format!(
                        "ACME order became invalid for '{}': {}. \
                         This usually means the HTTP-01 challenge failed. \
                         Ensure the domain is accessible at http://{}/.well-known/acme-challenge/",
                        domain, error_msg, domain
                    )
                    .into());
                }
                OrderStatus::Valid => {
                    // Already valid, can proceed to get certificate
                    tracing::info!(domain = %domain, "Order is already valid");
                    break;
                }
                OrderStatus::Pending | OrderStatus::Processing => {
                    // Still waiting, sleep and retry
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }

        // Step 5: Generate keypair and CSR with rcgen
        let key_pair = KeyPair::generate().map_err(|e| {
            tracing::error!(domain = %domain, error = %e, "Failed to generate key pair");
            format!("Failed to generate key pair for '{}': {}", domain, e)
        })?;

        let params = CertificateParams::new(vec![domain.to_string()]).map_err(|e| {
            tracing::error!(domain = %domain, error = %e, "Failed to create certificate params");
            format!(
                "Failed to create certificate params for '{}': {}",
                domain, e
            )
        })?;

        let csr = params.serialize_request(&key_pair).map_err(|e| {
            tracing::error!(domain = %domain, error = %e, "Failed to create CSR");
            format!("Failed to create CSR for '{}': {}", domain, e)
        })?;

        tracing::debug!(domain = %domain, "Generated CSR");

        // Step 6: Finalize order with CSR (only if not already valid)
        if order.state().status != OrderStatus::Valid {
            order.finalize(csr.der()).await.map_err(|e| {
                tracing::error!(domain = %domain, error = %e, "Failed to finalize order");
                format!("Failed to finalize order for '{}': {}", domain, e)
            })?;

            tracing::info!(domain = %domain, "Order finalized, waiting for certificate");
        }

        // Step 7: Get certificate (with timeout)
        let cert_chain = loop {
            if start_time.elapsed() > ACME_VALIDATION_TIMEOUT {
                self.clear_challenges_for_domain(domain).await;
                return Err(format!(
                    "Timeout waiting for certificate for '{}'. \
                     Certificate was not issued within {} seconds.",
                    domain,
                    ACME_VALIDATION_TIMEOUT.as_secs()
                )
                .into());
            }

            match order.certificate().await {
                Ok(Some(cert)) => break cert,
                Ok(None) => {
                    tracing::debug!(domain = %domain, "Certificate not yet available, waiting...");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Err(e) => {
                    self.clear_challenges_for_domain(domain).await;
                    return Err(format!("Failed to get certificate for '{}': {}", domain, e).into());
                }
            }
        };

        // Step 8: Store and return certificate
        let key_pem = key_pair.serialize_pem();
        self.store_cert(domain, &cert_chain, &key_pem).await?;
        self.clear_challenges_for_domain(domain).await;

        tracing::info!(
            domain = %domain,
            "Successfully provisioned certificate via ACME"
        );

        Ok((cert_chain, key_pem))
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

    // =========================================================================
    // Certificate Metadata Management
    // =========================================================================

    /// Parse certificate expiry dates from a PEM-encoded certificate
    ///
    /// # Arguments
    /// * `cert_pem` - The PEM-encoded certificate string
    ///
    /// # Returns
    /// Tuple of (not_before, not_after) as DateTime<Utc>
    pub fn parse_cert_expiry(
        cert_pem: &str,
    ) -> Result<(DateTime<Utc>, DateTime<Utc>), Box<dyn std::error::Error + Send + Sync>> {
        let (_, pem) =
            parse_x509_pem(cert_pem.as_bytes()).map_err(|e| format!("Failed to parse PEM: {e}"))?;

        let (_, cert) = x509_parser::parse_x509_certificate(&pem.contents)
            .map_err(|e| format!("Failed to parse X.509 certificate: {e}"))?;

        let validity = cert.validity();

        // Convert ASN1Time to DateTime<Utc>
        let not_before = DateTime::from_timestamp(validity.not_before.timestamp(), 0)
            .ok_or("Invalid not_before timestamp")?;

        let not_after = DateTime::from_timestamp(validity.not_after.timestamp(), 0)
            .ok_or("Invalid not_after timestamp")?;

        Ok((not_before, not_after))
    }

    /// Compute SHA256 fingerprint of a PEM-encoded certificate
    fn compute_cert_fingerprint(cert_pem: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(cert_pem.as_bytes());
        let result = hasher.finalize();
        hex::encode(result)
    }

    /// Save certificate metadata to disk
    ///
    /// # Arguments
    /// * `meta` - The certificate metadata to save
    async fn save_cert_metadata(
        &self,
        meta: &CertMetadata,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let meta_path = self.storage_path.join(format!("{}.meta.json", meta.domain));
        let json = serde_json::to_string_pretty(meta)?;
        tokio::fs::write(&meta_path, json).await?;
        tracing::debug!(domain = %meta.domain, "Saved certificate metadata");
        Ok(())
    }

    /// Load certificate metadata from disk
    ///
    /// # Arguments
    /// * `domain` - The domain to load metadata for
    ///
    /// # Returns
    /// The certificate metadata if it exists
    pub async fn load_cert_metadata(&self, domain: &str) -> Option<CertMetadata> {
        let meta_path = self.storage_path.join(format!("{}.meta.json", domain));
        if !meta_path.exists() {
            return None;
        }

        match tokio::fs::read_to_string(&meta_path).await {
            Ok(json) => match serde_json::from_str(&json) {
                Ok(meta) => Some(meta),
                Err(e) => {
                    tracing::warn!(
                        domain = %domain,
                        error = %e,
                        "Failed to parse certificate metadata"
                    );
                    None
                }
            },
            Err(e) => {
                tracing::warn!(
                    domain = %domain,
                    error = %e,
                    "Failed to read certificate metadata file"
                );
                None
            }
        }
    }

    /// Get domains with certificates expiring within a threshold
    ///
    /// Returns domains with certificates that expire within 30 days (RENEWAL_THRESHOLD_DAYS).
    ///
    /// # Returns
    /// Vector of domain names with certificates needing renewal
    pub async fn get_domains_needing_renewal(&self) -> Vec<String> {
        let threshold = Utc::now() + TimeDelta::days(RENEWAL_THRESHOLD_DAYS);
        let mut domains_needing_renewal = Vec::new();

        // Read directory for .meta.json files
        let mut entries = match tokio::fs::read_dir(&self.storage_path).await {
            Ok(entries) => entries,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to read certificate storage directory");
                return domains_needing_renewal;
            }
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                if filename.ends_with(".meta.json") {
                    let domain = filename.trim_end_matches(".meta.json");
                    if let Some(meta) = self.load_cert_metadata(domain).await {
                        if meta.not_after <= threshold {
                            tracing::debug!(
                                domain = %domain,
                                expires = %meta.not_after,
                                "Certificate needs renewal"
                            );
                            domains_needing_renewal.push(domain.to_string());
                        }
                    }
                }
            }
        }

        domains_needing_renewal
    }

    // =========================================================================
    // Automatic Certificate Renewal
    // =========================================================================

    /// Start background certificate renewal task
    ///
    /// This spawns a tokio task that checks certificates every 12 hours
    /// and renews any that expire within 30 days.
    ///
    /// # Arguments
    /// * `sni_resolver` - The SNI resolver to update with renewed certificates
    ///
    /// # Returns
    /// A `JoinHandle` for the spawned renewal task
    pub fn start_renewal_task(
        self: Arc<Self>,
        sni_resolver: Arc<SniCertResolver>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            // Check every 12 hours
            let mut interval = tokio::time::interval(Duration::from_secs(43200));

            loop {
                interval.tick().await;

                tracing::info!("Starting certificate renewal check");

                // Get domains needing renewal
                let domains = self.get_domains_needing_renewal().await;

                if domains.is_empty() {
                    tracing::debug!("No certificates need renewal");
                    continue;
                }

                tracing::info!(count = domains.len(), "Certificates need renewal");

                for domain in domains {
                    tracing::info!(domain = %domain, "Attempting certificate renewal");

                    match self.provision_cert(&domain).await {
                        Ok((cert_pem, key_pem)) => {
                            tracing::info!(domain = %domain, "Certificate renewed successfully");

                            // Update SNI resolver with new certificate
                            if let Err(e) = sni_resolver
                                .refresh_cert(&domain, &cert_pem, &key_pem)
                                .await
                            {
                                tracing::error!(
                                    domain = %domain,
                                    error = %e,
                                    "Failed to update SNI resolver with renewed cert"
                                );
                            }
                        }
                        Err(e) => {
                            tracing::error!(
                                domain = %domain,
                                error = %e,
                                "Certificate renewal failed"
                            );
                        }
                    }

                    // Small delay between renewals to avoid rate limiting
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
            }
        })
    }

    /// Run a single renewal check (for testing or on-demand renewal)
    ///
    /// This method checks for certificates needing renewal and attempts to renew them.
    /// Unlike `start_renewal_task`, this runs once and returns immediately.
    ///
    /// # Arguments
    /// * `sni_resolver` - The SNI resolver to update with renewed certificates
    ///
    /// # Returns
    /// Vector of domain names that were successfully renewed
    pub async fn run_renewal_check(&self, sni_resolver: &SniCertResolver) -> Vec<String> {
        let domains = self.get_domains_needing_renewal().await;
        let mut renewed = Vec::new();

        for domain in domains {
            match self.provision_cert(&domain).await {
                Ok((cert_pem, key_pem)) => {
                    if sni_resolver
                        .refresh_cert(&domain, &cert_pem, &key_pem)
                        .await
                        .is_ok()
                    {
                        renewed.push(domain);
                    }
                }
                Err(e) => {
                    tracing::warn!(domain = %domain, error = %e, "Renewal failed");
                }
            }
        }

        renewed
    }

    // =========================================================================
    // ACME Account Management
    // =========================================================================

    /// Get the path to the account metadata storage file
    fn account_path(&self) -> PathBuf {
        self.storage_path.join("account.json")
    }

    /// Get the path to the account credentials storage file
    fn credentials_path(&self) -> PathBuf {
        self.storage_path.join("account_credentials.json")
    }

    /// Load an existing ACME account from disk
    ///
    /// This loads the account metadata. Use `load_credentials()` to load the
    /// actual credentials needed for ACME operations.
    ///
    /// # Returns
    /// The account if it exists and is valid, None otherwise
    pub async fn load_account(&self) -> Option<AcmeAccount> {
        // Check if both files exist (need credentials to be usable)
        if !self.credentials_path().exists() {
            return None;
        }
        self.load_account_metadata().await
    }

    /// Load account metadata from disk
    async fn load_account_metadata(&self) -> Option<AcmeAccount> {
        let account_path = self.account_path();

        if !account_path.exists() {
            tracing::debug!(path = %account_path.display(), "No ACME account file found");
            return None;
        }

        match tokio::fs::read_to_string(&account_path).await {
            Ok(content) => match serde_json::from_str::<AcmeAccount>(&content) {
                Ok(account) => {
                    tracing::debug!(
                        account_url = %account.account_url,
                        created_at = %account.created_at,
                        "Loaded ACME account from disk"
                    );
                    Some(account)
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        path = %account_path.display(),
                        "Failed to parse ACME account file"
                    );
                    None
                }
            },
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    path = %account_path.display(),
                    "Failed to read ACME account file"
                );
                None
            }
        }
    }

    /// Load ACME credentials from disk
    ///
    /// Since AccountCredentials doesn't implement Clone, we load it fresh
    /// each time it's needed.
    async fn load_credentials(&self) -> Option<AccountCredentials> {
        let credentials_path = self.credentials_path();

        if !credentials_path.exists() {
            tracing::debug!(path = %credentials_path.display(), "No ACME credentials file found");
            return None;
        }

        match tokio::fs::read_to_string(&credentials_path).await {
            Ok(content) => match serde_json::from_str::<AccountCredentials>(&content) {
                Ok(credentials) => {
                    tracing::debug!("Loaded ACME credentials from disk");
                    Some(credentials)
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        path = %credentials_path.display(),
                        "Failed to parse ACME credentials file"
                    );
                    None
                }
            },
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    path = %credentials_path.display(),
                    "Failed to read ACME credentials file"
                );
                None
            }
        }
    }

    /// Save an ACME account to disk (metadata only)
    ///
    /// # Arguments
    /// * `account` - The account to save
    ///
    /// # Returns
    /// Ok(()) if successful, Err otherwise
    pub async fn save_account(
        &self,
        account: &AcmeAccount,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.save_account_metadata(account).await
    }

    /// Save account metadata to disk
    async fn save_account_metadata(
        &self,
        account: &AcmeAccount,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let account_path = self.account_path();

        let content = serde_json::to_string_pretty(account)?;
        tokio::fs::write(&account_path, content).await?;

        // Update the cached account
        *self.account.write().await = Some(account.clone());

        tracing::info!(
            account_url = %account.account_url,
            path = %account_path.display(),
            "Saved ACME account metadata to disk"
        );

        Ok(())
    }

    /// Save ACME credentials to disk
    async fn save_credentials(
        &self,
        credentials: &AccountCredentials,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let credentials_path = self.credentials_path();

        let content = serde_json::to_string_pretty(credentials)?;
        tokio::fs::write(&credentials_path, content).await?;

        tracing::info!(
            path = %credentials_path.display(),
            "Saved ACME credentials to disk"
        );

        Ok(())
    }

    /// Get or create an ACME account (returns our metadata struct)
    ///
    /// This method:
    /// 1. Returns the cached account if available
    /// 2. Loads the account from disk if it exists
    /// 3. Creates a new account via ACME
    ///
    /// # Returns
    /// The ACME account metadata
    pub async fn get_or_create_account(
        &self,
    ) -> Result<AcmeAccount, Box<dyn std::error::Error + Send + Sync>> {
        // Check cached account first
        {
            let account = self.account.read().await;
            if let Some(ref acc) = *account {
                return Ok(acc.clone());
            }
        }

        // Try to load from disk
        if let Some(account) = self.load_account().await {
            *self.account.write().await = Some(account.clone());
            return Ok(account);
        }

        // Create a new account (this will also cache it)
        let _account = self.get_or_create_acme_account().await?;
        let account_meta = self
            .account
            .read()
            .await
            .clone()
            .ok_or("Account was created but metadata not cached - this is a bug")?;

        Ok(account_meta)
    }

    /// Get or create an instant-acme Account object
    ///
    /// This is the internal method that returns the actual instant-acme Account
    /// needed for ACME operations. Since AccountCredentials doesn't implement Clone,
    /// we load credentials from disk each time.
    async fn get_or_create_acme_account(
        &self,
    ) -> Result<Account, Box<dyn std::error::Error + Send + Sync>> {
        // Try to load credentials from disk
        if let Some(credentials) = self.load_credentials().await {
            tracing::debug!("Restoring ACME account from saved credentials");

            let account = Account::from_credentials(credentials)
                .await
                .map_err(|e| format!("Failed to restore account from saved credentials: {}", e))?;

            // Ensure account metadata is cached
            if self.account.read().await.is_none() {
                if let Some(account_meta) = self.load_account_metadata().await {
                    *self.account.write().await = Some(account_meta);
                }
            }

            return Ok(account);
        }

        // Create a new account
        let email = self.acme_email.as_ref().ok_or(
            "ACME email is required to create a new account. \
             Please configure an email address for ACME registration.",
        )?;

        tracing::info!(
            email = %email,
            directory = %self.acme_directory,
            "Creating new ACME account"
        );

        let contact = format!("mailto:{}", email);
        let contact_refs: &[&str] = &[&contact];
        let new_account = NewAccount {
            contact: contact_refs,
            terms_of_service_agreed: true,
            only_return_existing: false,
        };

        let (account, credentials) = Account::create(&new_account, &self.acme_directory, None)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to create ACME account");
                format!("Failed to create ACME account: {}", e)
            })?;

        // Create our metadata struct
        let account_meta = AcmeAccount {
            account_url: account.id().to_string(),
            account_key_pem: String::new(), // We store credentials separately
            contact: vec![contact],
            created_at: Utc::now(),
        };

        // Save account and credentials to disk
        self.save_account_metadata(&account_meta).await?;
        self.save_credentials(&credentials).await?;

        // Cache the metadata
        *self.account.write().await = Some(account_meta.clone());

        tracing::info!(
            account_url = %account_meta.account_url,
            "Successfully created ACME account"
        );

        Ok(account)
    }

    /// Get the cached ACME account (if any)
    ///
    /// # Returns
    /// The cached account, or None if not loaded
    pub async fn get_account(&self) -> Option<AcmeAccount> {
        self.account.read().await.clone()
    }

    /// Check if an ACME account exists (either cached or on disk)
    pub async fn has_account(&self) -> bool {
        // Check cache first
        {
            let account = self.account.read().await;
            if account.is_some() {
                return true;
            }
        }

        // Check disk
        self.account_path().exists()
    }

    // =========================================================================
    // ACME HTTP-01 Challenge Management
    // =========================================================================

    /// Store an ACME HTTP-01 challenge token
    ///
    /// This stores the challenge token that will be served at
    /// `/.well-known/acme-challenge/{token}` for domain validation.
    ///
    /// # Arguments
    /// * `token` - The challenge token from the ACME server
    /// * `domain` - The domain being validated
    /// * `key_authorization` - The key authorization response (token.thumbprint)
    pub async fn store_challenge(&self, token: &str, domain: &str, key_authorization: &str) {
        let challenge = ChallengeToken {
            token: token.to_string(),
            key_authorization: key_authorization.to_string(),
            domain: domain.to_string(),
            created_at: Instant::now(),
        };
        self.challenges.insert(token.to_string(), challenge);
        tracing::debug!(
            token = %token,
            domain = %domain,
            "Stored ACME challenge token"
        );
    }

    /// Get the key authorization response for a challenge token
    ///
    /// # Arguments
    /// * `token` - The challenge token from the request path
    ///
    /// # Returns
    /// The key authorization string if the token exists and hasn't expired
    pub async fn get_challenge_response(&self, token: &str) -> Option<String> {
        self.challenges
            .get(token)
            .filter(|challenge| challenge.created_at.elapsed() < CHALLENGE_EXPIRATION)
            .map(|challenge| challenge.key_authorization.clone())
    }

    /// Remove a challenge token after validation completes
    ///
    /// # Arguments
    /// * `token` - The challenge token to remove
    pub async fn remove_challenge(&self, token: &str) {
        if self.challenges.remove(token).is_some() {
            tracing::debug!(token = %token, "Removed ACME challenge token");
        }
    }

    /// Clear all challenge tokens for a specific domain
    ///
    /// Useful when certificate issuance completes or fails for a domain.
    ///
    /// # Arguments
    /// * `domain` - The domain to clear challenges for
    pub async fn clear_challenges_for_domain(&self, domain: &str) {
        let tokens_to_remove: Vec<String> = self
            .challenges
            .iter()
            .filter(|entry| entry.value().domain == domain)
            .map(|entry| entry.key().clone())
            .collect();

        for token in &tokens_to_remove {
            self.challenges.remove(token);
        }

        if !tokens_to_remove.is_empty() {
            tracing::debug!(
                domain = %domain,
                count = tokens_to_remove.len(),
                "Cleared ACME challenge tokens for domain"
            );
        }
    }

    /// Clean up expired challenge tokens
    ///
    /// Removes all challenge tokens older than 5 minutes.
    /// This should be called periodically to prevent memory leaks.
    pub fn cleanup_expired_challenges(&self) {
        let expired_tokens: Vec<String> = self
            .challenges
            .iter()
            .filter(|entry| entry.value().created_at.elapsed() >= CHALLENGE_EXPIRATION)
            .map(|entry| entry.key().clone())
            .collect();

        for token in &expired_tokens {
            self.challenges.remove(token);
        }

        if !expired_tokens.is_empty() {
            tracing::debug!(
                count = expired_tokens.len(),
                "Cleaned up expired ACME challenge tokens"
            );
        }
    }

    /// Get the number of active challenge tokens
    pub fn challenge_count(&self) -> usize {
        self.challenges.len()
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

    #[tokio::test]
    async fn test_store_and_get_challenge() {
        let dir = tempdir().unwrap();
        let manager = CertManager::new(dir.path().to_string_lossy().to_string(), None)
            .await
            .unwrap();

        let token = "abc123";
        let domain = "test.example.com";
        let key_auth = "abc123.thumbprint";

        // Store a challenge
        manager.store_challenge(token, domain, key_auth).await;
        assert_eq!(manager.challenge_count(), 1);

        // Retrieve it
        let response = manager.get_challenge_response(token).await;
        assert_eq!(response, Some(key_auth.to_string()));

        // Non-existent token returns None
        let missing = manager.get_challenge_response("nonexistent").await;
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn test_remove_challenge() {
        let dir = tempdir().unwrap();
        let manager = CertManager::new(dir.path().to_string_lossy().to_string(), None)
            .await
            .unwrap();

        manager
            .store_challenge("token1", "domain.com", "auth1")
            .await;
        assert_eq!(manager.challenge_count(), 1);

        manager.remove_challenge("token1").await;
        assert_eq!(manager.challenge_count(), 0);

        // Removing non-existent token is a no-op
        manager.remove_challenge("nonexistent").await;
        assert_eq!(manager.challenge_count(), 0);
    }

    #[tokio::test]
    async fn test_clear_challenges_for_domain() {
        let dir = tempdir().unwrap();
        let manager = CertManager::new(dir.path().to_string_lossy().to_string(), None)
            .await
            .unwrap();

        // Store challenges for multiple domains
        manager
            .store_challenge("token1", "domain1.com", "auth1")
            .await;
        manager
            .store_challenge("token2", "domain1.com", "auth2")
            .await;
        manager
            .store_challenge("token3", "domain2.com", "auth3")
            .await;
        assert_eq!(manager.challenge_count(), 3);

        // Clear only domain1.com challenges
        manager.clear_challenges_for_domain("domain1.com").await;
        assert_eq!(manager.challenge_count(), 1);

        // domain2.com challenge should still exist
        let response = manager.get_challenge_response("token3").await;
        assert!(response.is_some());
    }

    #[tokio::test]
    async fn test_cleanup_expired_challenges() {
        let dir = tempdir().unwrap();
        let manager = CertManager::new(dir.path().to_string_lossy().to_string(), None)
            .await
            .unwrap();

        // Store a challenge
        manager
            .store_challenge("token1", "domain.com", "auth1")
            .await;
        assert_eq!(manager.challenge_count(), 1);

        // Cleanup should not remove fresh challenges
        manager.cleanup_expired_challenges();
        assert_eq!(manager.challenge_count(), 1);

        // Note: We can't easily test expiration without mocking time,
        // but the cleanup logic is tested by verifying it doesn't remove fresh tokens
    }

    // =========================================================================
    // ACME Account Persistence Tests
    // =========================================================================

    #[tokio::test]
    async fn test_save_and_load_account() {
        let dir = tempdir().unwrap();
        let manager = CertManager::new(dir.path().to_string_lossy().to_string(), None)
            .await
            .unwrap();

        // Initially no account
        assert!(!manager.has_account().await);
        assert!(manager.get_account().await.is_none());

        // Create and save an account
        let account = AcmeAccount {
            account_url: "https://acme.example.com/acct/12345".to_string(),
            account_key_pem: "-----BEGIN EC PRIVATE KEY-----\ntest\n-----END EC PRIVATE KEY-----"
                .to_string(),
            contact: vec!["mailto:test@example.com".to_string()],
            created_at: Utc::now(),
        };

        manager.save_account(&account).await.unwrap();

        // Should be cached
        assert!(manager.has_account().await);
        let cached = manager.get_account().await.unwrap();
        assert_eq!(cached.account_url, account.account_url);
        assert_eq!(cached.contact, account.contact);

        // Verify file exists on disk
        let account_path = dir.path().join("account.json");
        assert!(account_path.exists());
    }

    #[tokio::test]
    async fn test_load_account_from_disk() {
        let dir = tempdir().unwrap();

        // Create an account file manually
        let account = AcmeAccount {
            account_url: "https://acme.example.com/acct/67890".to_string(),
            account_key_pem: "-----BEGIN EC PRIVATE KEY-----\ntest\n-----END EC PRIVATE KEY-----"
                .to_string(),
            contact: vec!["mailto:admin@example.com".to_string()],
            created_at: Utc::now(),
        };

        let account_path = dir.path().join("account.json");
        let content = serde_json::to_string_pretty(&account).unwrap();
        std::fs::write(&account_path, content).unwrap();

        // Also need to create a mock credentials file for load_account to work
        // The credentials file must be valid JSON that deserializes to AccountCredentials
        // We use a minimal valid credentials structure here for testing
        let credentials_path = dir.path().join("account_credentials.json");
        // This is a minimal valid AccountCredentials JSON (the exact format is opaque but serializable)
        let mock_credentials = r#"{"id":"https://acme.example.com/acct/67890","key_pkcs8":"base64data","urls":{"newNonce":"https://acme.example.com/acme/new-nonce","newAccount":"https://acme.example.com/acme/new-account","newOrder":"https://acme.example.com/acme/new-order"}}"#;
        std::fs::write(&credentials_path, mock_credentials).unwrap();

        // Create manager - should load the account automatically
        let manager = CertManager::new(dir.path().to_string_lossy().to_string(), None)
            .await
            .unwrap();

        assert!(manager.has_account().await);
        let loaded = manager.get_account().await.unwrap();
        assert_eq!(loaded.account_url, account.account_url);
        assert_eq!(loaded.contact, account.contact);
    }

    #[tokio::test]
    async fn test_get_or_create_account_returns_cached() {
        let dir = tempdir().unwrap();
        let manager = CertManager::new(dir.path().to_string_lossy().to_string(), None)
            .await
            .unwrap();

        // Save an account first
        let account = AcmeAccount {
            account_url: "https://acme.example.com/acct/11111".to_string(),
            account_key_pem: "-----BEGIN EC PRIVATE KEY-----\ntest\n-----END EC PRIVATE KEY-----"
                .to_string(),
            contact: vec!["mailto:cached@example.com".to_string()],
            created_at: Utc::now(),
        };
        manager.save_account(&account).await.unwrap();

        // get_or_create should return the cached account
        let result = manager.get_or_create_account().await.unwrap();
        assert_eq!(result.account_url, account.account_url);
    }

    #[tokio::test]
    async fn test_get_or_create_account_without_existing() {
        let dir = tempdir().unwrap();
        // No email configured, so account creation will fail
        let manager = CertManager::new(dir.path().to_string_lossy().to_string(), None)
            .await
            .unwrap();

        // Should fail because no email is configured and no existing account
        let result = manager.get_or_create_account().await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("ACME email is required"),
            "Expected error about ACME email, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_load_invalid_account_file() {
        let dir = tempdir().unwrap();

        // Create an invalid JSON file
        let account_path = dir.path().join("account.json");
        std::fs::write(&account_path, "{ invalid json }").unwrap();

        // Manager should still be created, but no valid account should be loaded
        let manager = CertManager::new(dir.path().to_string_lossy().to_string(), None)
            .await
            .unwrap();

        // The file exists on disk, so has_account returns true (checking file existence)
        // But get_account returns None because no valid account was loaded into cache
        assert!(manager.get_account().await.is_none());

        // get_or_create_account should fail because:
        // 1. No cached account
        // 2. load_account returns None (invalid JSON)
        // 3. Account creation not implemented
        let result = manager.get_or_create_account().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_account_persistence_across_instances() {
        let dir = tempdir().unwrap();

        // Create first manager and save account with credentials
        {
            let manager = CertManager::new(dir.path().to_string_lossy().to_string(), None)
                .await
                .unwrap();

            let account = AcmeAccount {
                account_url: "https://acme.example.com/acct/persist".to_string(),
                account_key_pem:
                    "-----BEGIN EC PRIVATE KEY-----\npersist\n-----END EC PRIVATE KEY-----"
                        .to_string(),
                contact: vec!["mailto:persist@example.com".to_string()],
                created_at: Utc::now(),
            };
            manager.save_account(&account).await.unwrap();

            // Also need to create a mock credentials file for load_account to work
            let credentials_path = dir.path().join("account_credentials.json");
            let mock_credentials = r#"{"id":"https://acme.example.com/acct/persist","key_pkcs8":"base64data","urls":{"newNonce":"https://acme.example.com/acme/new-nonce","newAccount":"https://acme.example.com/acme/new-account","newOrder":"https://acme.example.com/acme/new-order"}}"#;
            std::fs::write(&credentials_path, mock_credentials).unwrap();
        }

        // Create second manager - should load the persisted account
        let manager2 = CertManager::new(dir.path().to_string_lossy().to_string(), None)
            .await
            .unwrap();

        assert!(manager2.has_account().await);
        let loaded = manager2.get_account().await.unwrap();
        assert_eq!(loaded.account_url, "https://acme.example.com/acct/persist");
    }

    // =========================================================================
    // Certificate Metadata Tests
    // =========================================================================

    /// Generate a valid test certificate using rcgen
    fn generate_test_cert() -> (String, String) {
        use rcgen::{CertificateParams, KeyPair};

        let key_pair = KeyPair::generate().unwrap();
        let params = CertificateParams::new(vec!["test.example.com".to_string()]).unwrap();
        let cert = params.self_signed(&key_pair).unwrap();
        (cert.pem(), key_pair.serialize_pem())
    }

    #[tokio::test]
    async fn test_parse_cert_expiry() {
        // Generate a valid test certificate
        let (cert_pem, _key_pem) = generate_test_cert();

        let result = CertManager::parse_cert_expiry(&cert_pem);
        assert!(result.is_ok(), "Failed to parse cert: {:?}", result.err());

        let (not_before, not_after) = result.unwrap();
        // The certificate should have valid timestamps
        assert!(not_before < not_after);
    }

    #[tokio::test]
    async fn test_parse_cert_expiry_invalid_pem() {
        let result = CertManager::parse_cert_expiry("not a valid pem");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cert_metadata_save_load() {
        let dir = tempdir().unwrap();
        let manager = CertManager::new(dir.path().to_string_lossy().to_string(), None)
            .await
            .unwrap();

        let meta = CertMetadata {
            domain: "test.example.com".to_string(),
            not_before: Utc::now(),
            not_after: Utc::now() + TimeDelta::days(90),
            provisioned_at: Utc::now(),
            fingerprint: "abc123def456".to_string(),
        };

        // Save metadata
        manager.save_cert_metadata(&meta).await.unwrap();

        // Verify file exists
        let meta_path = dir.path().join("test.example.com.meta.json");
        assert!(meta_path.exists());

        // Load metadata
        let loaded = manager.load_cert_metadata("test.example.com").await;
        assert!(loaded.is_some());

        let loaded = loaded.unwrap();
        assert_eq!(loaded.domain, "test.example.com");
        assert_eq!(loaded.fingerprint, "abc123def456");
    }

    #[tokio::test]
    async fn test_load_nonexistent_metadata() {
        let dir = tempdir().unwrap();
        let manager = CertManager::new(dir.path().to_string_lossy().to_string(), None)
            .await
            .unwrap();

        let loaded = manager.load_cert_metadata("nonexistent.example.com").await;
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn test_get_domains_needing_renewal() {
        let dir = tempdir().unwrap();
        let manager = CertManager::new(dir.path().to_string_lossy().to_string(), None)
            .await
            .unwrap();

        // Create a certificate that expires in 10 days (needs renewal)
        let expiring_meta = CertMetadata {
            domain: "expiring.example.com".to_string(),
            not_before: Utc::now() - TimeDelta::days(80),
            not_after: Utc::now() + TimeDelta::days(10),
            provisioned_at: Utc::now() - TimeDelta::days(80),
            fingerprint: "expiring_fingerprint".to_string(),
        };
        manager.save_cert_metadata(&expiring_meta).await.unwrap();

        // Create a certificate that expires in 60 days (does not need renewal)
        let valid_meta = CertMetadata {
            domain: "valid.example.com".to_string(),
            not_before: Utc::now() - TimeDelta::days(30),
            not_after: Utc::now() + TimeDelta::days(60),
            provisioned_at: Utc::now() - TimeDelta::days(30),
            fingerprint: "valid_fingerprint".to_string(),
        };
        manager.save_cert_metadata(&valid_meta).await.unwrap();

        // Check renewal list
        let needs_renewal = manager.get_domains_needing_renewal().await;
        assert_eq!(needs_renewal.len(), 1);
        assert!(needs_renewal.contains(&"expiring.example.com".to_string()));
    }

    #[tokio::test]
    async fn test_store_cert_with_valid_pem_saves_metadata() {
        let dir = tempdir().unwrap();
        let manager = CertManager::new(dir.path().to_string_lossy().to_string(), None)
            .await
            .unwrap();

        // Generate a valid test certificate
        let (cert_pem, key_pem) = generate_test_cert();

        // Store a valid certificate (this should also save metadata)
        manager
            .store_cert("metadata.example.com", &cert_pem, &key_pem)
            .await
            .unwrap();

        // Verify metadata was saved
        let meta = manager.load_cert_metadata("metadata.example.com").await;
        assert!(meta.is_some(), "Metadata was not saved");

        let meta = meta.unwrap();
        assert_eq!(meta.domain, "metadata.example.com");
        assert!(!meta.fingerprint.is_empty());
        assert!(meta.not_before < meta.not_after);
    }

    #[tokio::test]
    async fn test_store_cert_with_invalid_pem_still_stores_cert() {
        let dir = tempdir().unwrap();
        let manager = CertManager::new(dir.path().to_string_lossy().to_string(), None)
            .await
            .unwrap();

        // Store an invalid certificate (should still store cert, just skip metadata)
        let invalid_cert = "-----BEGIN CERTIFICATE-----\ninvalid\n-----END CERTIFICATE-----";
        let key = "-----BEGIN PRIVATE KEY-----\ntest\n-----END PRIVATE KEY-----";

        manager
            .store_cert("invalid.example.com", invalid_cert, key)
            .await
            .unwrap();

        // Certificate should still be stored
        assert!(manager.has_cert("invalid.example.com").await);

        // Metadata should not exist (parsing failed)
        let meta = manager.load_cert_metadata("invalid.example.com").await;
        assert!(meta.is_none());
    }

    #[tokio::test]
    async fn test_cert_fingerprint_is_consistent() {
        // Generate a test certificate
        let (cert_pem, _) = generate_test_cert();

        // Compute fingerprint twice, should be identical
        let fp1 = CertManager::compute_cert_fingerprint(&cert_pem);
        let fp2 = CertManager::compute_cert_fingerprint(&cert_pem);
        assert_eq!(fp1, fp2);

        // Different cert should have different fingerprint
        let (other_cert, _) = generate_test_cert();
        let fp3 = CertManager::compute_cert_fingerprint(&other_cert);
        assert_ne!(fp1, fp3);
    }

    // =========================================================================
    // ACME Provisioning Tests
    // =========================================================================

    #[tokio::test]
    async fn test_with_directory_staging() {
        let dir = tempdir().unwrap();
        let manager = CertManager::with_directory(
            dir.path().to_string_lossy().to_string(),
            Some("test@example.com".to_string()),
            super::LETS_ENCRYPT_STAGING.to_string(),
        )
        .await
        .unwrap();

        assert_eq!(manager.acme_directory(), super::LETS_ENCRYPT_STAGING);
    }

    #[tokio::test]
    async fn test_with_directory_production() {
        let dir = tempdir().unwrap();
        let manager = CertManager::with_directory(
            dir.path().to_string_lossy().to_string(),
            Some("test@example.com".to_string()),
            super::LETS_ENCRYPT_PRODUCTION.to_string(),
        )
        .await
        .unwrap();

        assert_eq!(manager.acme_directory(), super::LETS_ENCRYPT_PRODUCTION);
    }

    #[tokio::test]
    async fn test_default_uses_production() {
        let dir = tempdir().unwrap();
        let manager = CertManager::new(
            dir.path().to_string_lossy().to_string(),
            Some("test@example.com".to_string()),
        )
        .await
        .unwrap();

        assert_eq!(manager.acme_directory(), super::LETS_ENCRYPT_PRODUCTION);
    }

    #[tokio::test]
    async fn test_provision_cert_requires_email() {
        let dir = tempdir().unwrap();
        // No email configured
        let manager = CertManager::new(dir.path().to_string_lossy().to_string(), None)
            .await
            .unwrap();

        // Try to get a cert that doesn't exist (will trigger provisioning)
        let result = manager.get_cert("test.example.com").await;
        assert!(result.is_err());

        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("ACME email is required"),
            "Expected error about ACME email, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_credentials_path() {
        let dir = tempdir().unwrap();
        let manager = CertManager::new(dir.path().to_string_lossy().to_string(), None)
            .await
            .unwrap();

        let credentials_path = manager.credentials_path();
        assert_eq!(
            credentials_path,
            dir.path().join("account_credentials.json")
        );
    }

    #[tokio::test]
    async fn test_load_credentials_nonexistent() {
        let dir = tempdir().unwrap();
        let manager = CertManager::new(dir.path().to_string_lossy().to_string(), None)
            .await
            .unwrap();

        let credentials = manager.load_credentials().await;
        assert!(credentials.is_none());
    }

    // =========================================================================
    // Automatic Renewal Tests
    // =========================================================================

    #[tokio::test]
    async fn test_start_renewal_task_spawns() {
        use crate::sni_resolver::SniCertResolver;

        let dir = tempdir().unwrap();
        let manager = Arc::new(
            CertManager::new(dir.path().to_string_lossy().to_string(), None)
                .await
                .unwrap(),
        );
        let sni_resolver = Arc::new(SniCertResolver::new());

        // Start the renewal task
        let handle = manager.clone().start_renewal_task(sni_resolver);

        // The task should be running (not immediately finished)
        assert!(!handle.is_finished());

        // Abort the task to clean up
        handle.abort();
    }

    #[tokio::test]
    async fn test_run_renewal_check_no_certs() {
        use crate::sni_resolver::SniCertResolver;

        let dir = tempdir().unwrap();
        let manager = CertManager::new(dir.path().to_string_lossy().to_string(), None)
            .await
            .unwrap();
        let sni_resolver = SniCertResolver::new();

        // No certificates stored, should return empty
        let renewed = manager.run_renewal_check(&sni_resolver).await;
        assert!(renewed.is_empty());
    }

    #[tokio::test]
    async fn test_run_renewal_check_with_fresh_cert() {
        use crate::sni_resolver::SniCertResolver;

        let dir = tempdir().unwrap();
        let manager = CertManager::new(dir.path().to_string_lossy().to_string(), None)
            .await
            .unwrap();
        let sni_resolver = SniCertResolver::new();

        // Create a certificate that does NOT need renewal (expires in 60 days)
        let meta = CertMetadata {
            domain: "fresh.example.com".to_string(),
            not_before: Utc::now() - TimeDelta::days(30),
            not_after: Utc::now() + TimeDelta::days(60),
            provisioned_at: Utc::now() - TimeDelta::days(30),
            fingerprint: "fresh_fingerprint".to_string(),
        };
        manager.save_cert_metadata(&meta).await.unwrap();

        // Also store the actual cert so it exists
        let (cert_pem, key_pem) = generate_test_cert();
        manager
            .store_cert("fresh.example.com", &cert_pem, &key_pem)
            .await
            .unwrap();

        // Run renewal check - should not attempt renewal since cert is fresh
        let renewed = manager.run_renewal_check(&sni_resolver).await;
        assert!(
            renewed.is_empty(),
            "Expected no renewals for fresh cert, got: {:?}",
            renewed
        );
    }
}
