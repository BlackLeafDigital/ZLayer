//! Native Windows builder backend using HCS (Host Compute Service).
//!
//! This backend builds Windows container images on a Windows host without
//! requiring Docker Desktop or buildah. The heavy lifting — base image
//! pull/materialisation, per-instruction execution against an ephemeral
//! HCS compute system, NTFS-diff capture via `BackupRead`, OCI manifest
//! emission, and registry push — lives in [`crate::windows_builder`]
//! ([`WindowsBuilder::build_image_for_backend`]). This module is now a
//! thin [`BuildBackend`] adapter that constructs a [`WindowsBuilder`]
//! per build and delegates to it.
//!
//! # Deferred
//!
//! - **Standalone `push_image` by tag** — `WindowsBuilder`'s push path is
//!   invoked inline by `build_image_for_backend` when `options.push` is
//!   set. A standalone retag/push that operates on an already-built image
//!   by tag (without rebuilding) is not yet wired through; see
//!   `TODO(W-followup)` on the [`BuildBackend::push_image`] impl below.
//! - **Multi-platform manifest lists** (`manifest_create` /
//!   `manifest_add` / `manifest_push`) remain `NotSupported` — they are a
//!   buildah-specific concept and the HCS builder produces a single
//!   platform-specific image. Use a Linux peer with buildah for
//!   multi-platform manifest composition.

// `#[cfg(target_os = "windows")]` is applied by the parent `backend/mod.rs`
// module declaration, so it is not repeated here.
#![allow(clippy::items_after_test_module)]

mod commit;

pub use commit::{
    build_image_config_bytes, build_manifest_bytes, ImageConfigBuilder,
    OCI_IMAGE_CONFIG_MEDIA_TYPE, OCI_IMAGE_MANIFEST_MEDIA_TYPE, OCI_WINDOWS_LAYER_MEDIA_TYPE,
};

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use async_trait::async_trait;

use crate::builder::{BuildOptions, BuiltImage, RegistryAuth};
use crate::dockerfile::Dockerfile;
use crate::error::{BuildError, Result};
use crate::tui::BuildEvent;
use crate::windows_builder::{WindowsBuildConfig, WindowsBuilder};

use super::BuildBackend;

/// Root directory under the user's local app data where the HCS builder
/// stages scratch layers, pulled base-image chains, and written OCI blobs.
///
/// Resolved via [`dirs::data_local_dir`] with a last-resort fallback to
/// `C:\ProgramData\zlayer\builder-hcs` when the per-user dir is unavailable
/// (for example when running inside a Windows Service session that has no
/// profile folder).
fn default_storage_root() -> PathBuf {
    if let Some(dir) = dirs::data_local_dir() {
        dir.join("zlayer").join("builder-hcs")
    } else {
        PathBuf::from(r"C:\ProgramData\zlayer\builder-hcs")
    }
}

/// Native Windows build backend.
///
/// Holds the on-disk scratch root only — every `build_image` call
/// constructs a fresh [`WindowsBuilder`] rooted under
/// `storage_root/builds/<build-id>/` and delegates to
/// [`WindowsBuilder::build_image_for_backend`].
pub struct HcsBackend {
    /// Root directory for per-build state.
    storage_root: PathBuf,
}

impl std::fmt::Debug for HcsBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HcsBackend")
            .field("storage_root", &self.storage_root)
            .finish_non_exhaustive()
    }
}

impl HcsBackend {
    /// Construct a new HCS backend with default storage root.
    ///
    /// # Errors
    ///
    /// Returns an error if the storage root cannot be created.
    pub async fn new() -> Result<Self> {
        let root = default_storage_root();
        Self::with_storage_root(root).await
    }

    /// Construct a new HCS backend with an explicit storage root.
    ///
    /// # Errors
    ///
    /// Returns an error if the storage root cannot be created.
    // `async` is kept for API stability — callers `.await` the constructor
    // and a future revision may need to do real async work here (e.g.
    // probing HCS service availability with a background task).
    #[allow(clippy::unused_async)]
    pub async fn with_storage_root(storage_root: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&storage_root).map_err(|e| BuildError::ContextRead {
            path: storage_root.clone(),
            source: e,
        })?;
        Ok(Self { storage_root })
    }

    /// Where per-build artifacts for a specific build id live on disk.
    fn build_dir(&self, build_id: &str) -> PathBuf {
        self.storage_root.join("builds").join(build_id)
    }
}

#[async_trait]
impl BuildBackend for HcsBackend {
    async fn build_image(
        &self,
        context: &Path,
        dockerfile: &Dockerfile,
        options: &BuildOptions,
        event_tx: Option<mpsc::Sender<BuildEvent>>,
    ) -> Result<BuiltImage> {
        // Per-build cache directory under this backend's storage root.
        let build_id = new_build_id();
        let cache_dir = self.build_dir(&build_id);
        std::fs::create_dir_all(&cache_dir).map_err(|e| BuildError::ContextRead {
            path: cache_dir.clone(),
            source: e,
        })?;

        // Map the public BuildOptions auth into the registry-side auth
        // shape WindowsBuildConfig expects. Anonymous is the safe default
        // for unauthenticated MCR / GHCR public pulls.
        let registry_auth = match options.registry_auth.as_ref() {
            Some(a) => zlayer_registry::RegistryAuth::Basic(a.username.clone(), a.password.clone()),
            None => zlayer_registry::RegistryAuth::Anonymous,
        };

        let cfg = WindowsBuildConfig {
            cache_dir,
            registry_auth,
            platform: WindowsBuildConfig::default_platform().to_string(),
            os_version_override: None,
            scratch_size_gb: WindowsBuildConfig::default_scratch_size_gb(),
        };

        let builder = WindowsBuilder::new(cfg);
        builder
            .build_image_for_backend(context, dockerfile, options, event_tx.as_ref())
            .await
    }

    async fn push_image(&self, _tag: &str, _auth: Option<&RegistryAuth>) -> Result<()> {
        // WindowsBuilder's push path is invoked inline by
        // build_image_for_backend when options.push is true; a
        // standalone push by tag (without rebuilding) is not yet wired
        // through — see TODO(W-followup).
        Err(BuildError::NotSupported {
            operation: "HCS backend standalone push by tag — push is performed inline by \
                        build_image when options.push is set; standalone push without a \
                        rebuild is not yet wired (TODO(W-followup))"
                .to_string(),
        })
    }

    async fn tag_image(&self, _image: &str, _new_tag: &str) -> Result<()> {
        // The OCI artifacts written by `write_oci_artifacts` record the tag
        // embedded in the index annotation; a retag post-build is a pure
        // manifest-index rewrite. Exposing a knob for it lands with the push
        // path above.
        Err(BuildError::NotSupported {
            operation: "HCS backend retag — tags are embedded in the OCI index annotations at \
                        build time; a standalone retag lands with the push path \
                        (TODO(W-followup))"
                .to_string(),
        })
    }

    async fn manifest_create(&self, _name: &str) -> Result<()> {
        Err(BuildError::NotSupported {
            operation: "HCS backend manifest create — manifest lists are buildah-specific; the \
                        HCS builder produces a single platform-specific image. Use a Linux peer \
                        with buildah for multi-platform manifest composition."
                .to_string(),
        })
    }

    async fn manifest_add(&self, _manifest: &str, _image: &str) -> Result<()> {
        Err(BuildError::NotSupported {
            operation: "HCS backend manifest add — see manifest_create for rationale".to_string(),
        })
    }

    async fn manifest_push(&self, _name: &str, _destination: &str) -> Result<()> {
        Err(BuildError::NotSupported {
            operation: "HCS backend manifest push — see manifest_create for rationale".to_string(),
        })
    }

    async fn is_available(&self) -> bool {
        // HCS is present on any supported Windows Server / desktop build; we
        // rely on `cfg(target_os = "windows")` at compile time. A deeper probe
        // would open a dummy operation handle, but that carries measurable
        // cost at every detect_backend() call so we keep this cheap.
        true
    }

    fn name(&self) -> &'static str {
        "hcs"
    }
}

/// Generate a fresh build identifier. Matches the shape the buildah backend
/// uses (12-hex chars from a time + pid + counter hash) so log lines line up
/// across backends.
fn new_build_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = u128::from(std::process::id());
    let count = u128::from(COUNTER.fetch_add(1, Ordering::Relaxed));
    let mixed = nanos ^ (pid.rotate_left(17)) ^ count.rotate_left(33);
    format!("{:012x}", mixed & 0xFFFF_FFFF_FFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_id_is_unique_and_hex_shaped() {
        let a = new_build_id();
        let b = new_build_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 12);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn default_storage_root_ends_in_zlayer_builder_hcs() {
        let root = default_storage_root();
        let s = root.to_string_lossy().to_ascii_lowercase();
        assert!(s.contains("zlayer"));
        assert!(s.contains("builder-hcs"));
    }
}
