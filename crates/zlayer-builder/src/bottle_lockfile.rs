//! Per-spec bottle lockfile for reproducible macOS Homebrew installs.
//!
//! When a sandbox build resolves Linux package names to Homebrew bottles, the
//! exact bottle URLs (which embed sha256 digests on GHCR) and dependency graph
//! are captured in `zlayer-bottles.lock` next to the spec/Dockerfile. Subsequent
//! builds load the lockfile and short-circuit `resolve_package`, returning the
//! pinned URL/version directly instead of re-querying brew. This makes the
//! macOS bottle path deterministic across builds and across time, even if
//! upstream brew rotates a formula's default version.
//!
//! # Lifecycle
//!
//! - **Generate**: first build (no lockfile) runs the live `resolve_package`
//!   flow and records every bottle it touches into a `Vec<LockedBottle>`. The
//!   builder writes `<spec_dir>/zlayer-bottles.lock` after the build succeeds.
//! - **Consume**: subsequent builds load the lockfile up front. `resolve_package`
//!   checks the lockfile before any HTTP work; a hit synthesizes a
//!   `ResolvedPackage::HomebrewBottle` from the locked entry.
//! - **Update**: `zlayer build --update-bottles` ignores any existing lockfile,
//!   forces the live path for every formula, and rewrites the file. Mirrors
//!   `cargo update`.
//!
//! # Why URL pins are sufficient
//!
//! Homebrew bottles are served from `ghcr.io/v2/homebrew/core/.../blobs/sha256:<digest>`
//! — the URL is content-addressed. Pinning the URL pins the exact bytes; we
//! don't need a separate sha256 field for bottles served from GHCR. For the
//! rare non-GHCR case (older mirrors, taps), `resolve_package` already accepts
//! the live URL without sha verification, so the lockfile matches that surface.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::error::{BuildError, Result};

/// File name written next to the spec/Dockerfile.
pub const LOCKFILE_NAME: &str = "zlayer-bottles.lock";

/// Current schema version. Bump if the on-disk format changes incompatibly.
pub const CURRENT_SCHEMA: u32 = 1;

/// Top-level lockfile document.
///
/// Stored as TOML at `<spec_dir>/zlayer-bottles.lock`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BottleLockfile {
    /// Schema version. Mismatched versions cause a load error so the build
    /// fails loudly rather than silently using a stale layout.
    pub schema: u32,
    /// ISO-8601 UTC timestamp of when this file was written.
    pub generated_at: String,
    /// One entry per resolved bottle. Order is stable (sorted by formula name)
    /// so diffs are reviewable.
    #[serde(default, rename = "bottle")]
    pub bottles: Vec<LockedBottle>,
}

/// A single resolved Homebrew bottle pinned to specific download URLs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockedBottle {
    /// Canonical brew formula name, e.g. `openssl@3`.
    pub formula: String,
    /// Stable version string from the formula API at lock time, e.g. `3.6.2`.
    pub version: String,
    /// Direct dependency formula names, in declaration order. Used to drive
    /// the BFS in `install_with_deps` without re-querying brew.
    #[serde(default)]
    pub deps: Vec<String>,
    /// Per-platform-tag bottle download URLs. Keys match
    /// `macos_image_resolver::bottle_platform_tag()` output (e.g.
    /// `arm64_sequoia`, `sonoma`) plus `all` for noarch bottles. Storing every
    /// platform brew published lets one lockfile work across macOS versions
    /// inside a team.
    pub urls: HashMap<String, String>,
}

impl BottleLockfile {
    /// Build a fresh empty lockfile with the current schema and timestamp.
    #[must_use]
    pub fn new() -> Self {
        Self {
            schema: CURRENT_SCHEMA,
            generated_at: now_iso8601(),
            bottles: Vec::new(),
        }
    }

    /// Load and parse the lockfile from disk. Returns `Ok(None)` when the
    /// file does not exist (fresh-build case); returns `Err` when the file
    /// exists but cannot be read or parsed.
    ///
    /// # Errors
    ///
    /// Filesystem read failure or TOML parse failure.
    pub async fn load(path: &Path) -> Result<Option<Self>> {
        let text = match tokio::fs::read_to_string(path).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(BuildError::IoError(std::io::Error::other(format!(
                    "failed to read {}: {e}",
                    path.display()
                ))));
            }
        };

        let parsed: Self = toml::from_str(&text).map_err(|e| BuildError::RegistryError {
            message: format!("failed to parse {}: {e}", path.display()),
        })?;

        if parsed.schema != CURRENT_SCHEMA {
            return Err(BuildError::RegistryError {
                message: format!(
                    "{} schema {} not supported (expected {}); regenerate with --update-bottles",
                    path.display(),
                    parsed.schema,
                    CURRENT_SCHEMA,
                ),
            });
        }

        debug!(
            "Loaded {} pinned bottles from {}",
            parsed.bottles.len(),
            path.display()
        );
        Ok(Some(parsed))
    }

    /// Atomically write the lockfile to disk via tmp-then-rename so a crash
    /// mid-write can't leave a corrupted file in place.
    ///
    /// # Errors
    ///
    /// Filesystem write failure or TOML serialization failure.
    pub async fn save(&self, path: &Path) -> Result<()> {
        let mut sorted = self.clone();
        sorted.bottles.sort_by(|a, b| a.formula.cmp(&b.formula));

        let text = toml::to_string_pretty(&sorted).map_err(|e| BuildError::RegistryError {
            message: format!("failed to serialize lockfile: {e}"),
        })?;

        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| BuildError::RegistryError {
                message: format!("failed to create {}: {e}", parent.display()),
            })?;

        let tmp = path.with_extension("lock.tmp");
        tokio::fs::write(&tmp, text)
            .await
            .map_err(|e| BuildError::RegistryError {
                message: format!("failed to write {}: {e}", tmp.display()),
            })?;
        tokio::fs::rename(&tmp, path)
            .await
            .map_err(|e| BuildError::RegistryError {
                message: format!(
                    "failed to rename {} -> {}: {e}",
                    tmp.display(),
                    path.display()
                ),
            })?;
        debug!(
            "Wrote {} pinned bottles to {}",
            sorted.bottles.len(),
            path.display()
        );
        Ok(())
    }

    /// Look up a formula by name. Returns the pinned entry if present.
    #[must_use]
    pub fn get(&self, formula: &str) -> Option<&LockedBottle> {
        self.bottles.iter().find(|b| b.formula == formula)
    }

    /// Insert or replace an entry. Lookup is by `formula` name.
    pub fn upsert(&mut self, entry: LockedBottle) {
        if let Some(slot) = self.bottles.iter_mut().find(|b| b.formula == entry.formula) {
            *slot = entry;
        } else {
            self.bottles.push(entry);
        }
    }
}

impl Default for BottleLockfile {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve the lockfile path for a given spec or Dockerfile path. The
/// lockfile lives sibling to the spec, like `Cargo.lock` next to `Cargo.toml`.
///
/// If `spec_path` is a directory, the lockfile path is `<dir>/zlayer-bottles.lock`.
/// If it's a file, the lockfile is in the file's parent directory.
#[must_use]
pub fn lockfile_path_for(spec_path: &Path) -> PathBuf {
    let dir = if spec_path.is_dir() {
        spec_path.to_path_buf()
    } else {
        spec_path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    };
    dir.join(LOCKFILE_NAME)
}

/// Current UTC time in ISO-8601 (`YYYY-MM-DDTHH:MM:SSZ`). Used for the
/// `generated_at` stamp on writes and matches the format the package-map
/// generator emits. Uses the workspace's `chrono` (already used in
/// `zlayer-agent` and elsewhere) instead of hand-rolling the date math.
fn now_iso8601() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Issue a one-shot warning when an existing lockfile is ignored. Helper kept
/// here so the call sites in `sandbox_builder` stay terse.
pub fn warn_lockfile_ignored(path: &Path, reason: &str) {
    warn!("Ignoring lockfile at {}: {}", path.display(), reason);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_empty() {
        let lock = BottleLockfile::new();
        let text = toml::to_string_pretty(&lock).unwrap();
        let parsed: BottleLockfile = toml::from_str(&text).unwrap();
        assert_eq!(parsed.schema, CURRENT_SCHEMA);
        assert!(parsed.bottles.is_empty());
    }

    #[test]
    fn roundtrip_with_entries() {
        let mut lock = BottleLockfile::new();
        let mut urls = HashMap::new();
        urls.insert(
            "arm64_sequoia".to_string(),
            "https://example/openssl@3".to_string(),
        );
        urls.insert(
            "all".to_string(),
            "https://example/openssl@3-all".to_string(),
        );
        lock.upsert(LockedBottle {
            formula: "openssl@3".to_string(),
            version: "3.6.2".to_string(),
            deps: vec!["ca-certificates".to_string()],
            urls,
        });

        let text = toml::to_string_pretty(&lock).unwrap();
        assert!(text.contains("openssl@3"));
        let parsed: BottleLockfile = toml::from_str(&text).unwrap();
        assert_eq!(parsed.bottles.len(), 1);
        assert_eq!(parsed.bottles[0].deps, vec!["ca-certificates".to_string()]);
    }

    #[test]
    fn lockfile_path_sibling_to_file() {
        let p = lockfile_path_for(Path::new("/tmp/myapp/Dockerfile"));
        assert_eq!(p, PathBuf::from("/tmp/myapp/zlayer-bottles.lock"));
    }

    #[test]
    fn lockfile_path_for_directory() {
        // Real path; relies on /tmp existing.
        let p = lockfile_path_for(Path::new("/tmp"));
        assert_eq!(p, PathBuf::from("/var/lib/zlayer-bottles.lock"));
    }
}
