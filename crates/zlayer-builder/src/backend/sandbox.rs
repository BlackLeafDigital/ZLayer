//! macOS Seatbelt sandbox build backend.
//!
//! Uses [`SandboxImageBuilder`] when buildah is not available on macOS.

use std::path::{Path, PathBuf};

use crate::builder::{BuildOptions, BuiltImage, RegistryAuth};
use crate::dockerfile::Dockerfile;
use crate::error::{BuildError, Result};
use crate::sandbox_builder::SandboxImageBuilder;
use crate::tui::BuildEvent;

use super::BuildBackend;

/// macOS sandbox build backend.
///
/// Builds images using the native macOS Seatbelt sandbox, producing a rootfs
/// directory + `config.json` rather than OCI images. This backend does not
/// support push, tag, or manifest operations.
pub struct SandboxBackend {
    /// Base data directory for storing images (e.g. `~/.zlayer`).
    data_dir: PathBuf,
}

impl SandboxBackend {
    /// Create a new sandbox backend with the given data directory.
    #[must_use]
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }
}

impl Default for SandboxBackend {
    fn default() -> Self {
        let data_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp/zlayer"))
            .join("zlayer");
        Self { data_dir }
    }
}

#[async_trait::async_trait]
impl BuildBackend for SandboxBackend {
    async fn build_image(
        &self,
        context: &Path,
        dockerfile: &Dockerfile,
        options: &BuildOptions,
        event_tx: Option<std::sync::mpsc::Sender<BuildEvent>>,
    ) -> Result<BuiltImage> {
        let mut builder = SandboxImageBuilder::new(context.to_path_buf(), self.data_dir.clone());

        // Transfer build args
        if !options.build_args.is_empty() {
            builder = builder.with_build_args(options.build_args.clone());
        }

        // Attach event channel if provided
        if let Some(tx) = event_tx {
            builder = builder.with_events(tx);
        }

        // Run the build
        let result = builder.build(dockerfile, &options.tags).await?;

        // Convert SandboxBuildResult → BuiltImage
        Ok(BuiltImage {
            image_id: result.image_id,
            tags: result.tags,
            layer_count: 1, // sandbox builds produce a single rootfs
            size: 0,        // not computed by sandbox builder
            build_time_ms: result.build_time_ms,
            is_manifest: false,
        })
    }

    async fn push_image(&self, _tag: &str, _auth: Option<&RegistryAuth>) -> Result<()> {
        Err(BuildError::NotSupported {
            operation: "push".into(),
        })
    }

    async fn tag_image(&self, _image: &str, _new_tag: &str) -> Result<()> {
        Err(BuildError::NotSupported {
            operation: "tag".into(),
        })
    }

    async fn manifest_create(&self, _name: &str) -> Result<()> {
        Err(BuildError::NotSupported {
            operation: "manifest_create".into(),
        })
    }

    async fn manifest_add(&self, _manifest: &str, _image: &str) -> Result<()> {
        Err(BuildError::NotSupported {
            operation: "manifest_add".into(),
        })
    }

    async fn manifest_push(&self, _name: &str, _destination: &str) -> Result<()> {
        Err(BuildError::NotSupported {
            operation: "manifest_push".into(),
        })
    }

    async fn is_available(&self) -> bool {
        // The sandbox backend is always available on macOS
        true
    }

    fn name(&self) -> &'static str {
        "sandbox"
    }
}
