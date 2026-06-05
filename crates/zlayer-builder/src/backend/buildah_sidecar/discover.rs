//! Discovery for the `zlayer-buildd` sidecar binary.
//!
//! Resolves the path of the Go sidecar in this order:
//!
//!  1. `ZLAYER_BUILDD_BIN` env var (absolute path; used to bypass discovery
//!     for development / CI / unit tests).
//!  2. `${ZLAYER_DATA_DIR}/bin/zlayer-buildd` — the canonical install
//!     location written by `crates/zlayer-builder/src/buildah/install.rs`
//!     (task 3.5).
//!  3. `$PATH` lookup — distro packaging or operator-installed binary.
//!  4. Fail with `BuildError::NotSupported` whose message names every path
//!     that was tried, so the operator can see exactly where the binary
//!     was expected.

use std::env;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::error::{BuildError, Result};

const BIN_NAME: &str = "zlayer-buildd";
const ENV_OVERRIDE: &str = "ZLAYER_BUILDD_BIN";

/// Result of a discovery probe — records which candidate was selected and
/// the full list of paths the search visited (for diagnostics).
#[derive(Debug, Clone)]
pub struct Discovery {
    /// Absolute path to the discovered `zlayer-buildd` binary.
    pub binary: PathBuf,
    /// Every candidate path inspected during discovery, in probe order.
    pub tried: Vec<PathBuf>,
}

/// Resolve the location of `zlayer-buildd` on this host.
///
/// `dirs_resolver` is invoked lazily — only called if the env override
/// lookup fails. This keeps unit tests from needing a configured
/// `ZLayerDirs`. Pass [`default_data_dir`] or a closure that returns a
/// `Some(PathBuf)` to plug in test data.
///
/// # Errors
///
/// Returns [`BuildError::NotSupported`] when no executable named
/// `zlayer-buildd` is found in any of the probed locations. The error
/// message lists every candidate path that was tried.
pub fn discover<F>(dirs_resolver: F) -> Result<Discovery>
where
    F: FnOnce() -> Option<PathBuf>,
{
    let mut tried = Vec::new();

    // 1) Env override.
    if let Some(path) = env::var_os(ENV_OVERRIDE) {
        let path = PathBuf::from(path);
        tried.push(path.clone());
        if is_executable(&path) {
            return Ok(Discovery {
                binary: path,
                tried,
            });
        }
    }

    // 2) ${data_dir}/bin/zlayer-buildd.
    if let Some(data_dir) = dirs_resolver() {
        let candidate = data_dir.join("bin").join(BIN_NAME);
        tried.push(candidate.clone());
        if is_executable(&candidate) {
            return Ok(Discovery {
                binary: candidate,
                tried,
            });
        }
    }

    // 3) $PATH search.
    if let Some(path_env) = env::var_os("PATH") {
        for dir in env::split_paths(&path_env) {
            let candidate = dir.join(BIN_NAME);
            tried.push(candidate.clone());
            if is_executable(&candidate) {
                return Ok(Discovery {
                    binary: candidate,
                    tried,
                });
            }
        }
    }

    Err(BuildError::NotSupported {
        operation: format!(
            "buildah-sidecar backend: `{BIN_NAME}` binary not found. Set {ENV_OVERRIDE} \
             to an absolute path, install via `zlayer install --sidecar`, or add it to \
             $PATH. Tried: {tried}",
            tried = tried
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    })
}

/// Convenience wrapper that probes using the system-default
/// [`zlayer_paths::ZLayerDirs`] data directory.
///
/// # Errors
///
/// Same as [`discover`].
pub fn discover_default() -> Result<Discovery> {
    discover(default_data_dir)
}

/// Returns the canonical `ZLayer` data directory (cached on first call) for
/// use as the `dirs_resolver` argument to [`discover`].
#[must_use]
pub fn default_data_dir() -> Option<PathBuf> {
    cached_data_dir().cloned()
}

fn cached_data_dir() -> Option<&'static PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        Some(
            zlayer_paths::ZLayerDirs::system_default()
                .data_dir()
                .to_path_buf(),
        )
    })
    .as_ref()
}

fn is_executable(path: &Path) -> bool {
    use std::fs;
    match fs::metadata(path) {
        Ok(md) if md.is_file() => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                md.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            {
                true
            }
        }
        _ => false,
    }
}

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use super::*;
    use crate::TEST_ENV_LOCK;
    use std::fs;

    #[cfg(unix)]
    fn write_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        fs::write(path, b"#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
    }

    #[cfg(not(unix))]
    fn write_executable(path: &Path) {
        fs::write(path, b"exit 0\n").unwrap();
    }

    #[test]
    fn env_override_wins() {
        let _g = TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("custom-buildd");
        write_executable(&bin);
        // SAFETY: env mutation serialized by `TEST_ENV_LOCK`.
        unsafe {
            env::set_var(ENV_OVERRIDE, &bin);
        }
        let result = discover(|| None).unwrap();
        // SAFETY: env mutation serialized by `TEST_ENV_LOCK`.
        unsafe {
            env::remove_var(ENV_OVERRIDE);
        }
        assert_eq!(result.binary, bin);
    }

    #[test]
    fn data_dir_candidate_used_when_env_unset() {
        let _g = TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // SAFETY: env mutation serialized by `TEST_ENV_LOCK`.
        unsafe {
            env::remove_var(ENV_OVERRIDE);
        }
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("bin")).unwrap();
        let bin = tmp.path().join("bin").join(BIN_NAME);
        write_executable(&bin);
        let result = discover(|| Some(tmp.path().to_path_buf())).unwrap();
        assert_eq!(result.binary, bin);
    }

    #[test]
    fn errors_with_diagnostic_when_missing() {
        let _g = TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let prev_path = env::var_os("PATH");
        // SAFETY: env mutation serialized by `TEST_ENV_LOCK`.
        unsafe {
            env::remove_var(ENV_OVERRIDE);
            env::set_var("PATH", "/nonexistent-zlayer-test-dir");
        }
        let result = discover(|| None);
        // SAFETY: restore PATH before releasing the lock so subsequent
        // tests (e.g. `test_find_in_path_exists`) see the real PATH.
        unsafe {
            match prev_path {
                Some(v) => env::set_var("PATH", v),
                None => env::remove_var("PATH"),
            }
        }
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains(BIN_NAME), "error did not name binary: {msg}");
        assert!(
            msg.contains("Tried"),
            "error did not list tried paths: {msg}"
        );
    }
}
