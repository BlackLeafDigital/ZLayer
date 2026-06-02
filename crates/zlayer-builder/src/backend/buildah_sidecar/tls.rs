//! mTLS material lifecycle for the zlayer-buildd sidecar.
//!
//! On first use, generates a self-signed CA + leaf cert + private key pair
//! under `${ZLAYER_DATA_DIR}/buildd/`. Subsequent runs reuse the same
//! material. Permissions are locked to 0600 on Unix.
//!
//! The same `ca.pem` is used as both the *server's* `--tls-ca` (verifies
//! the client) and the *client's* CA bundle. Because the CA also signed
//! the leaf, server verification just works. This is intentional: the
//! sidecar is a strictly local-trust component; we are not federating
//! with any external CA.

use std::fs;
use std::path::{Path, PathBuf};

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, SanType,
};

use crate::error::{BuildError, Result};

/// Paths to the on-disk mTLS material used by the buildah sidecar.
///
/// Both the local zlayer daemon (client side) and the spawned
/// `zlayer-buildd` (server side) read the *same* PEMs — the CA verifies
/// the leaf in both directions, so mTLS works without any second
/// certificate authority.
#[derive(Debug, Clone)]
pub struct TlsMaterial {
    /// Directory holding `ca.pem`, `cert.pem`, and `key.pem`.
    pub dir: PathBuf,
    /// PEM-encoded self-signed CA cert.
    pub ca_pem: PathBuf,
    /// PEM-encoded leaf cert (signed by the CA, used by both client +
    /// server roles thanks to dual EKUs).
    pub cert_pem: PathBuf,
    /// PEM-encoded private key matching `cert.pem`.
    pub key_pem: PathBuf,
}

impl TlsMaterial {
    /// Construct path layout for a TLS material directory without
    /// touching the filesystem.
    #[must_use]
    pub fn under(dir: PathBuf) -> Self {
        Self {
            ca_pem: dir.join("ca.pem"),
            cert_pem: dir.join("cert.pem"),
            key_pem: dir.join("key.pem"),
            dir,
        }
    }

    /// `true` iff all three PEM files are regular files on disk.
    #[must_use]
    pub fn exists(&self) -> bool {
        self.ca_pem.is_file() && self.cert_pem.is_file() && self.key_pem.is_file()
    }
}

/// Ensure mTLS material exists at `dir`. Generates a fresh CA + leaf if
/// any file is missing. Idempotent: re-running with all three PEMs in
/// place is a cheap stat + return.
///
/// # Errors
///
/// Returns [`BuildError::IoError`] if the directory cannot be created or
/// the PEMs cannot be written; returns [`BuildError::NotSupported`]
/// (carrying the rcgen failure message) if certificate or key
/// generation fails.
///
/// # Panics
///
/// Panics only if the hard-coded loopback IP literals (`127.0.0.1`,
/// `::1`) fail to parse — that would indicate a broken standard
/// library, not a runtime input error.
pub fn ensure_tls_material(dir: &Path) -> Result<TlsMaterial> {
    let mat = TlsMaterial::under(dir.to_path_buf());
    if mat.exists() {
        return Ok(mat);
    }

    fs::create_dir_all(&mat.dir).map_err(BuildError::from)?;
    set_dir_mode_0700(&mat.dir)?;

    // 1) CA — self-signed.
    let mut ca_params =
        CertificateParams::new(vec!["zlayer-buildd-ca".into()]).map_err(|e| rcgen_err(&e))?;
    let mut ca_dn = DistinguishedName::new();
    ca_dn.push(DnType::CommonName, "zlayer-buildd-ca");
    ca_params.distinguished_name = ca_dn;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca_key = KeyPair::generate().map_err(|e| rcgen_err(&e))?;
    let ca_cert = ca_params.self_signed(&ca_key).map_err(|e| rcgen_err(&e))?;

    // 2) Leaf — both client + server EKU, with localhost+IP SANs so it
    // verifies under any reasonable rustls SNI / IP-name check.
    let mut leaf_params = CertificateParams::new(vec!["zlayer-buildd".into(), "localhost".into()])
        .map_err(|e| rcgen_err(&e))?;
    let mut leaf_dn = DistinguishedName::new();
    leaf_dn.push(DnType::CommonName, "zlayer-buildd");
    leaf_params.distinguished_name = leaf_dn;
    leaf_params.extended_key_usages = vec![
        ExtendedKeyUsagePurpose::ServerAuth,
        ExtendedKeyUsagePurpose::ClientAuth,
    ];
    leaf_params.subject_alt_names.push(SanType::IpAddress(
        "127.0.0.1".parse().expect("constant valid IPv4"),
    ));
    leaf_params.subject_alt_names.push(SanType::IpAddress(
        "::1".parse().expect("constant valid IPv6"),
    ));
    let leaf_key = KeyPair::generate().map_err(|e| rcgen_err(&e))?;
    let leaf_cert = leaf_params
        .signed_by(&leaf_key, &ca_cert, &ca_key)
        .map_err(|e| rcgen_err(&e))?;

    fs::write(&mat.ca_pem, ca_cert.pem()).map_err(BuildError::from)?;
    fs::write(&mat.cert_pem, leaf_cert.pem()).map_err(BuildError::from)?;
    fs::write(&mat.key_pem, leaf_key.serialize_pem()).map_err(BuildError::from)?;

    set_file_mode_0600(&mat.ca_pem)?;
    set_file_mode_0600(&mat.cert_pem)?;
    set_file_mode_0600(&mat.key_pem)?;

    Ok(mat)
}

fn rcgen_err(e: &rcgen::Error) -> BuildError {
    BuildError::NotSupported {
        operation: format!("sidecar mTLS generation: {e}"),
    }
}

#[cfg(unix)]
fn set_file_mode_0600(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_file_mode_0600(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_dir_mode_0700(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o700);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_dir_mode_0700(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_creates_pems_idempotently() {
        let tmp = tempfile::tempdir().unwrap();
        let m1 = ensure_tls_material(tmp.path()).unwrap();
        assert!(m1.exists(), "first call must produce all three PEMs");
        let first = fs::read(&m1.ca_pem).unwrap();

        let m2 = ensure_tls_material(tmp.path()).unwrap();
        let second = fs::read(&m2.ca_pem).unwrap();
        assert_eq!(first, second, "ca.pem must be stable across calls");
        assert_eq!(m1.dir, m2.dir);
    }

    #[test]
    fn ensure_writes_under_supplied_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("nested").join("buildd");
        let mat = ensure_tls_material(&sub).unwrap();
        assert!(mat.ca_pem.starts_with(&sub));
        assert!(mat.cert_pem.starts_with(&sub));
        assert!(mat.key_pem.starts_with(&sub));
    }

    #[cfg(unix)]
    #[test]
    fn pem_files_are_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let mat = ensure_tls_material(tmp.path()).unwrap();
        for p in [&mat.ca_pem, &mat.cert_pem, &mat.key_pem] {
            let mode = fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode,
                0o600,
                "{} should be 0600 (got {:o})",
                p.display(),
                mode
            );
        }
    }
}
