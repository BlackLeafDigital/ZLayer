//! Build backend abstraction.
//!
//! Provides a unified [`BuildBackend`] trait that decouples the build orchestration
//! logic from the underlying container tooling. Platform-specific implementations
//! are selected at runtime via [`detect_backend`].
//!
//! # Backends
//!
//! - [`BuildahBackend`] — wraps the `buildah` CLI (Linux + macOS with buildah installed).
//! - [`SandboxBackend`] — macOS-only, uses the Seatbelt sandbox when buildah is unavailable.

mod buildah;
#[cfg(target_os = "macos")]
mod sandbox;

pub use buildah::BuildahBackend;
#[cfg(target_os = "macos")]
pub use sandbox::SandboxBackend;

use std::path::Path;
use std::sync::Arc;

use crate::builder::{BuildOptions, BuiltImage, RegistryAuth};
use crate::dockerfile::Dockerfile;
use crate::error::{BuildError, Result};
use crate::tui::BuildEvent;

/// A pluggable build backend.
///
/// Implementations handle the low-level mechanics of building, pushing, tagging,
/// and managing manifest lists for container images.
#[async_trait::async_trait]
pub trait BuildBackend: Send + Sync {
    /// Build a container image from a parsed Dockerfile.
    ///
    /// # Arguments
    ///
    /// * `context`    — path to the build context directory
    /// * `dockerfile` — parsed Dockerfile IR
    /// * `options`    — build configuration (tags, args, caching, etc.)
    /// * `event_tx`   — optional channel for streaming progress events to a TUI
    async fn build_image(
        &self,
        context: &Path,
        dockerfile: &Dockerfile,
        options: &BuildOptions,
        event_tx: Option<std::sync::mpsc::Sender<BuildEvent>>,
    ) -> Result<BuiltImage>;

    /// Push an image to a container registry.
    async fn push_image(&self, tag: &str, auth: Option<&RegistryAuth>) -> Result<()>;

    /// Tag an existing image with a new name.
    async fn tag_image(&self, image: &str, new_tag: &str) -> Result<()>;

    /// Create a new (empty) manifest list.
    async fn manifest_create(&self, name: &str) -> Result<()>;

    /// Add an image to an existing manifest list.
    async fn manifest_add(&self, manifest: &str, image: &str) -> Result<()>;

    /// Push a manifest list (and all referenced images) to a registry.
    async fn manifest_push(&self, name: &str, destination: &str) -> Result<()>;

    /// Returns `true` if the backend tooling is installed and functional.
    async fn is_available(&self) -> bool;

    /// Human-readable name for this backend (e.g. `"buildah"`, `"sandbox"`).
    fn name(&self) -> &'static str;
}

/// Auto-detect the best available build backend for the current platform.
///
/// Selection order:
///
/// 1. If the `ZLAYER_BACKEND` environment variable is set to `"buildah"` or
///    `"sandbox"`, that backend is forced.
/// 2. On macOS, buildah is tried first; if unavailable, the sandbox backend
///    is returned as a fallback.
/// 3. On Linux, only the buildah backend is available.
///
/// # Errors
///
/// Returns an error if no usable backend can be found.
pub async fn detect_backend() -> Result<Arc<dyn BuildBackend>> {
    // Check for explicit override
    if let Ok(forced) = std::env::var("ZLAYER_BACKEND") {
        match forced.to_lowercase().as_str() {
            "buildah" => {
                let backend = BuildahBackend::new().await?;
                return Ok(Arc::new(backend));
            }
            #[cfg(target_os = "macos")]
            "sandbox" => {
                let backend = SandboxBackend::default();
                return Ok(Arc::new(backend));
            }
            other => {
                return Err(BuildError::BuildahNotFound {
                    message: format!("Unknown ZLAYER_BACKEND value: {other}"),
                });
            }
        }
    }

    // Auto-detect
    #[cfg(target_os = "macos")]
    {
        if let Ok(backend) = BuildahBackend::try_new().await {
            Ok(Arc::new(backend))
        } else {
            tracing::info!("Buildah not available on macOS, falling back to sandbox backend");
            Ok(Arc::new(SandboxBackend::default()))
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let backend = BuildahBackend::new().await?;
        Ok(Arc::new(backend))
    }
}
