//! Worker-CA support for `ZLayer`'s worker-tier mTLS.
//!
//! When a cluster operates in `worker-tier` mode, control-plane nodes maintain
//! an X.509 Certificate Authority used to issue short-lived mTLS leaf certs to
//! worker nodes during their `Register` RPC. Workers generate an EC P-256
//! keypair locally, build a PKCS#10 CSR, and submit it; the leader verifies
//! the bootstrap token, signs the CSR with this CA, and returns the cert +
//! the CA chain.
//!
//! Workers then mutually-authenticate every subsequent gRPC connection using
//! the leaf cert; the control plane validates against the same CA's root.
//!
//! # On-disk layout
//!
//! Two files under the cluster's data directory:
//!
//! - `<base_dir>/worker_ca.crt` — CA cert in PEM (mode 0644, world-readable
//!   because it's the root of the public chain).
//! - `<base_dir>/worker_ca.key` — CA private key in PEM PKCS#8 (mode 0600).
//!
//! Generation is one-shot: if `worker_ca.crt`/`worker_ca.key` already exist on
//! disk, they're loaded; otherwise a fresh P-256 self-signed CA is generated
//! and persisted atomically (`*.tmp` → `rename`).

use std::path::{Path, PathBuf};

use rcgen::{
    BasicConstraints, Certificate, CertificateParams, CertificateSigningRequestParams,
    DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose,
    PKCS_ECDSA_P256_SHA256,
};
use rustls_pki_types::CertificateSigningRequestDer;
use time::{Duration, OffsetDateTime};
use tracing::{debug, info};

use crate::{Result, SecretsError};

/// File name of the CA certificate (PEM, mode 0644).
pub const WORKER_CA_CERT_FILE: &str = "worker_ca.crt";
/// File name of the CA private key (PEM PKCS#8, mode 0600).
pub const WORKER_CA_KEY_FILE: &str = "worker_ca.key";

/// Default leaf-cert validity (90 days). Workers must re-register before this
/// expires; the control plane should rotate well in advance.
pub const DEFAULT_LEAF_VALIDITY_DAYS: i64 = 90;

/// Default CA-cert validity (10 years). The CA is long-lived; rotation is
/// a separate (manual, future) op.
pub const DEFAULT_CA_VALIDITY_YEARS: i64 = 10;

/// Worker certificate authority.
///
/// Holds the CA keypair + cert in memory after load/generate. Persisted to
/// disk so the CA identity survives daemon restarts.
pub struct WorkerCa {
    cert: Certificate,
    key_pair: KeyPair,
    base_dir: PathBuf,
}

impl std::fmt::Debug for WorkerCa {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerCa")
            .field("base_dir", &self.base_dir)
            .field("subject", &self.cert.params().distinguished_name)
            .finish_non_exhaustive()
    }
}

impl WorkerCa {
    /// Load the worker CA from `base_dir`, generating one if absent.
    ///
    /// # Errors
    ///
    /// Returns [`SecretsError::Storage`] on I/O failure and
    /// [`SecretsError::Encryption`] on malformed on-disk PEM or `rcgen`
    /// errors.
    pub fn load_or_generate(base_dir: impl AsRef<Path>) -> Result<Self> {
        let base_dir = base_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&base_dir)
            .map_err(|e| SecretsError::Storage(format!("create worker CA dir: {e}")))?;

        let cert_path = base_dir.join(WORKER_CA_CERT_FILE);
        let key_path = base_dir.join(WORKER_CA_KEY_FILE);

        if cert_path.exists() && key_path.exists() {
            return Self::load_from_pem(&cert_path, &key_path, base_dir);
        }

        Self::generate_and_persist(base_dir)
    }

    fn load_from_pem(cert_path: &Path, key_path: &Path, base_dir: PathBuf) -> Result<Self> {
        let cert_pem = std::fs::read_to_string(cert_path).map_err(|e| {
            SecretsError::Storage(format!("read worker CA cert {}: {e}", cert_path.display()))
        })?;
        let key_pem = std::fs::read_to_string(key_path).map_err(|e| {
            SecretsError::Storage(format!("read worker CA key {}: {e}", key_path.display()))
        })?;

        let key_pair = KeyPair::from_pem(&key_pem)
            .map_err(|e| SecretsError::Encryption(format!("parse worker CA key PEM: {e}")))?;

        let params = CertificateParams::from_ca_cert_pem(&cert_pem)
            .map_err(|e| SecretsError::Encryption(format!("parse worker CA cert PEM: {e}")))?;
        let cert = params
            .self_signed(&key_pair)
            .map_err(|e| SecretsError::Encryption(format!("re-bind CA cert: {e}")))?;

        debug!("Loaded existing worker CA from {}", base_dir.display());
        Ok(Self {
            cert,
            key_pair,
            base_dir,
        })
    }

    fn generate_and_persist(base_dir: PathBuf) -> Result<Self> {
        let mut params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "ZLayer Worker CA");
        dn.push(DnType::OrganizationName, "ZLayer");
        params.distinguished_name = dn;
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];

        let now = OffsetDateTime::now_utc();
        params.not_before = now - Duration::minutes(1);
        params.not_after = now + Duration::days(DEFAULT_CA_VALIDITY_YEARS * 365);

        let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
            .map_err(|e| SecretsError::Encryption(format!("generate worker CA keypair: {e}")))?;
        let cert = params
            .self_signed(&key_pair)
            .map_err(|e| SecretsError::Encryption(format!("self-sign worker CA cert: {e}")))?;

        let cert_pem = cert.pem();
        let key_pem = key_pair.serialize_pem();

        let cert_path = base_dir.join(WORKER_CA_CERT_FILE);
        let key_path = base_dir.join(WORKER_CA_KEY_FILE);

        atomic_write(&cert_path, cert_pem.as_bytes(), 0o644)?;
        atomic_write(&key_path, key_pem.as_bytes(), 0o600)?;

        info!(
            "Generated new worker CA at {} (valid {} years)",
            base_dir.display(),
            DEFAULT_CA_VALIDITY_YEARS
        );

        Ok(Self {
            cert,
            key_pair,
            base_dir,
        })
    }

    /// Return the CA certificate in DER encoding for inclusion in the gRPC
    /// `RegisterResponse.ca_chain_der`.
    #[must_use]
    pub fn ca_cert_der(&self) -> Vec<u8> {
        self.cert.der().to_vec()
    }

    /// Return the CA certificate in PEM (for human readers / debug).
    #[must_use]
    pub fn ca_cert_pem(&self) -> String {
        self.cert.pem()
    }

    /// Sign a worker-submitted CSR. Returns the leaf cert in DER.
    ///
    /// # Errors
    ///
    /// Returns [`SecretsError::Encryption`] if the CSR is malformed, uses an
    /// unsupported key type, or signing fails.
    pub fn sign_csr_der(
        &self,
        csr_der: &[u8],
        common_name: &str,
        validity: Duration,
    ) -> Result<Vec<u8>> {
        // Convert raw DER bytes to the typed `CertificateSigningRequestDer`
        // that rcgen 0.13 expects. The borrowed-slice `From` impl avoids
        // copying the bytes.
        let csr_typed = CertificateSigningRequestDer::from(csr_der);

        let mut params = CertificateSigningRequestParams::from_der(&csr_typed)
            .map_err(|e| SecretsError::Encryption(format!("parse CSR: {e}")))?;

        // Override the subject CN to a leader-controlled value so a malicious
        // worker can't pick its own identity. Everything else (key, SANs) comes
        // from the CSR as submitted.
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, common_name);
        dn.push(DnType::OrganizationName, "ZLayer Worker");
        params.params.distinguished_name = dn;

        let now = OffsetDateTime::now_utc();
        params.params.not_before = now - Duration::minutes(1);
        params.params.not_after = now + validity;
        params.params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        params.params.extended_key_usages = vec![
            ExtendedKeyUsagePurpose::ClientAuth,
            ExtendedKeyUsagePurpose::ServerAuth,
        ];

        let cert = params
            .signed_by(&self.cert, &self.key_pair)
            .map_err(|e| SecretsError::Encryption(format!("sign CSR: {e}")))?;

        Ok(cert.der().to_vec())
    }

    /// Base directory where the CA files live.
    #[must_use]
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }
}

fn atomic_write(path: &Path, data: &[u8], mode: u32) -> Result<()> {
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|s| s.to_str()).unwrap_or("")
    ));
    std::fs::write(&tmp, data)
        .map_err(|e| SecretsError::Storage(format!("write tmp {}: {e}", tmp.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(mode);
        std::fs::set_permissions(&tmp, perms)
            .map_err(|e| SecretsError::Storage(format!("chmod {}: {e}", tmp.display())))?;
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        SecretsError::Storage(format!(
            "rename {} -> {}: {e}",
            tmp.display(),
            path.display()
        ))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn load_or_generate_persists_and_reloads() {
        let dir = TempDir::new().expect("tempdir");
        let ca1 = WorkerCa::load_or_generate(dir.path()).expect("generate");
        let der1 = ca1.ca_cert_der();
        drop(ca1);

        let ca2 = WorkerCa::load_or_generate(dir.path()).expect("reload");
        let der2 = ca2.ca_cert_der();

        // ECDSA signatures are non-deterministic — re-signing on reload
        // produces a fresh signature even with the same key. The cert body
        // (everything up to the signature) and the subject public-key info
        // must match.
        let (_, cert1) = x509_parser::parse_x509_certificate(&der1).expect("parse cert1");
        let (_, cert2) = x509_parser::parse_x509_certificate(&der2).expect("parse cert2");
        assert_eq!(
            cert1.tbs_certificate.subject_pki.subject_public_key.data,
            cert2.tbs_certificate.subject_pki.subject_public_key.data,
            "reload must yield same CA public key"
        );
        assert_eq!(
            cert1.tbs_certificate.subject.to_string(),
            cert2.tbs_certificate.subject.to_string(),
            "reload must yield same CA subject"
        );
    }

    #[test]
    fn sign_csr_round_trip() {
        let dir = TempDir::new().expect("tempdir");
        let ca = WorkerCa::load_or_generate(dir.path()).expect("ca");

        // Worker side: generate a P-256 keypair + CSR.
        let worker_kp = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("kp");
        let mut csr_params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "doesnt-matter-leader-overrides");
        csr_params.distinguished_name = dn;
        let csr = csr_params
            .serialize_request(&worker_kp)
            .expect("serialize CSR");
        let csr_der = csr.der().to_vec();

        // Leader signs.
        let leaf_der = ca
            .sign_csr_der(&csr_der, "worker-7", Duration::days(7))
            .expect("sign");

        assert!(!leaf_der.is_empty());
        // x509-parser sanity check: the signed cert has our CA as issuer.
        let (_, parsed) = x509_parser::parse_x509_certificate(&leaf_der).expect("parse leaf");
        let issuer_cn = parsed
            .issuer()
            .iter_common_name()
            .next()
            .and_then(|cn| cn.as_str().ok())
            .unwrap_or("");
        assert_eq!(issuer_cn, "ZLayer Worker CA");

        let subject_cn = parsed
            .subject()
            .iter_common_name()
            .next()
            .and_then(|cn| cn.as_str().ok())
            .unwrap_or("");
        assert_eq!(subject_cn, "worker-7");
    }
}
