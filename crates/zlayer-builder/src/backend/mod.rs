//! Build backend abstraction.
//!
//! Provides a unified [`BuildBackend`] trait that decouples the build orchestration
//! logic from the underlying container tooling. Platform-specific implementations
//! are selected at runtime via [`detect_backend`].
//!
//! # Backends
//!
//! - [`BuildahBackend`] — wraps the `buildah` CLI (Linux + macOS with buildah installed).
//! - `SandboxBackend` (macOS-only) — uses the Seatbelt sandbox when buildah is unavailable.
//! - `HcsBackend` (Windows-only, see [`hcs`]) — native Windows builder via HCS;
//!   wraps `zlayer_agent::windows::{scratch, layer}` to produce OCI images
//!   without Docker Desktop.
//!
//! # Target OS routing
//!
//! The [`ImageOs`] enum selects which *image* OS we are building for (Linux or
//! Windows). [`detect_backend`] branches on both the host OS and the target OS:
//! Windows images can only be built on a Windows host (via the HCS-backed
//! backend that landed in Phase L-4), while Linux images on Windows hosts
//! currently require a Linux peer (a WSL2-buildah route is a Phase L
//! follow-up).

mod buildah;
pub mod buildah_sidecar;
#[cfg(target_os = "windows")]
pub mod hcs;
#[cfg(target_os = "macos")]
mod sandbox;

pub use buildah::BuildahBackend;
pub use buildah_sidecar::BuildahSidecarBackend;
#[cfg(target_os = "windows")]
pub use hcs::HcsBackend;
#[cfg(target_os = "macos")]
pub use sandbox::SandboxBackend;

use std::path::Path;
use std::sync::Arc;

use crate::builder::{BuildOptions, BuiltImage, RegistryAuth};
use crate::dockerfile::Dockerfile;
use crate::error::{BuildError, Result};
use crate::tui::BuildEvent;

/// Operating system of the image being built.
///
/// This is distinct from the host OS — a Linux host can only build Linux
/// images, a Windows host is required to build Windows images (via the
/// HCS-backed backend landing in Phase L-4).
///
/// Serializes to lowercase (`"linux"` / `"windows"`) for YAML/JSON configs.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum ImageOs {
    #[default]
    Linux,
    Windows,
}

/// Error returned when parsing an unknown [`ImageOs`] string.
#[derive(thiserror::Error, Debug)]
#[error("unknown OS: {0} (expected linux or windows)")]
pub struct ImageOsParseError(pub String);

impl std::str::FromStr for ImageOs {
    type Err = ImageOsParseError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        // Accept bare OS names ("linux", "Windows") AND platform-style strings
        // ("linux/amd64", "windows/arm64"). Split on '/' first and match the
        // OS component case-insensitively.
        let os_part = s.split('/').next().unwrap_or("").trim();
        match os_part.to_ascii_lowercase().as_str() {
            "linux" => Ok(ImageOs::Linux),
            "windows" => Ok(ImageOs::Windows),
            _ => Err(ImageOsParseError(s.to_string())),
        }
    }
}

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

/// Auto-detect the best available build backend for the given target OS.
///
/// Selection matrix (host × target):
///
/// | Host / Target | Linux image                               | Windows image                             |
/// |---------------|-------------------------------------------|-------------------------------------------|
/// | Linux         | buildah                                   | Err — requires Windows host               |
/// | macOS         | buildah (if available) else macos-sandbox | Err — requires Windows host               |
/// | Windows       | Err — Linux peer required (WSL2 follow-up)| HCS-backed native Windows builder (L-4)   |
///
/// If the `ZLAYER_BACKEND` env var is set to `"buildah"` or (on macOS)
/// `"sandbox"`, that backend is forced regardless of target OS.
///
/// # Errors
///
/// Returns an error if the host cannot build images for the requested
/// `target_os`, or if the selected backend's tooling is missing.
pub async fn detect_backend(target_os: ImageOs) -> Result<Arc<dyn BuildBackend>> {
    // Check for explicit override first — respected regardless of target_os so
    // devs can force a backend during debugging.
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

    // Host × target routing.
    #[cfg(target_os = "windows")]
    {
        match target_os {
            ImageOs::Linux => Err(BuildError::BuildahNotFound {
                message: "Linux image building on Windows hosts requires a Linux peer \
                          (Phase L follow-up will add WSL2-buildah routing)"
                    .to_string(),
            }),
            ImageOs::Windows => {
                let backend = HcsBackend::new().await?;
                Ok(Arc::new(backend))
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        match target_os {
            ImageOs::Linux => {
                if let Ok(backend) = BuildahBackend::try_new().await {
                    Ok(Arc::new(backend))
                } else {
                    tracing::info!(
                        "Buildah not available on macOS, falling back to sandbox backend"
                    );
                    Ok(Arc::new(SandboxBackend::default()))
                }
            }
            ImageOs::Windows => Err(BuildError::BuildahNotFound {
                message: "building Windows images requires a Windows host — run this build \
                          on a Windows node of the ZLayer cluster"
                    .to_string(),
            }),
        }
    }

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        match target_os {
            ImageOs::Linux => {
                let backend = BuildahBackend::new().await?;
                Ok(Arc::new(backend))
            }
            ImageOs::Windows => Err(BuildError::BuildahNotFound {
                message: "building Windows images requires a Windows host — run this build \
                          on a Windows node of the ZLayer cluster"
                    .to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_os_parses_simple_and_slash_form() {
        assert_eq!("linux".parse::<ImageOs>().unwrap(), ImageOs::Linux);
        assert_eq!("Linux".parse::<ImageOs>().unwrap(), ImageOs::Linux);
        assert_eq!("windows".parse::<ImageOs>().unwrap(), ImageOs::Windows);
        assert_eq!("linux/amd64".parse::<ImageOs>().unwrap(), ImageOs::Linux);
        assert_eq!(
            "windows/amd64".parse::<ImageOs>().unwrap(),
            ImageOs::Windows
        );
        assert!("darwin".parse::<ImageOs>().is_err());
    }
}
