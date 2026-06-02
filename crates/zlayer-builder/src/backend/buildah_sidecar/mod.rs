//! gRPC-client backend that talks to a `zlayer-buildd` sidecar.
//!
//! Spawns or connects to a `zlayer-buildd` process (Go binary wrapping
//! `imagebuildah.BuildDockerfiles`) and exchanges build requests / streamed
//! events over TCP+mTLS.
//!
//! Lifecycle, mTLS setup, and request translation are implemented across
//! tasks 3.1–3.5. This module currently contains only the skeleton + the
//! `BuildBackend` impl that returns `NotSupported` so Stage 1 compiles
//! cleanly.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;

use crate::backend::BuildBackend;
use crate::builder::{BuildOptions, BuiltImage, RegistryAuth};
use crate::dockerfile::Dockerfile;
use crate::error::{BuildError, Result};
use crate::tui::BuildEvent;

/// Generated tonic + prost bindings for `proto/buildah_sidecar.proto`.
pub mod proto {
    #![allow(clippy::all, missing_docs, clippy::pedantic, clippy::nursery)]
    tonic::include_proto!("zlayer.buildah_sidecar.v1");
}

/// gRPC-client backend for the `zlayer-buildd` sidecar.
///
/// Construction does not connect or spawn — that happens lazily on the
/// first `build_image` call once tasks 3.1–3.5 land. The current
/// implementation is a placeholder that:
///
/// - holds the resolved `SidecarConfig` (transport address, TLS dir, idle
///   timeout),
/// - reports itself unavailable so `detect_backend` falls back to the CLI
///   path,
/// - returns `BuildError::NotSupported` from every trait method.
#[derive(Debug, Clone)]
pub struct BuildahSidecarBackend {
    config: Arc<zlayer_types::builder::SidecarConfig>,
}

impl BuildahSidecarBackend {
    /// Stable trait-level name. Mirrors `BuildBackend::name`.
    pub const NAME: &'static str = "buildah-sidecar";

    /// Create a new sidecar backend with the supplied configuration.
    #[must_use]
    pub fn new(config: zlayer_types::builder::SidecarConfig) -> Self {
        Self {
            config: Arc::new(config),
        }
    }

    /// Borrow the underlying configuration. Used by lifecycle code in task
    /// 3.2.
    #[must_use]
    pub fn config(&self) -> &zlayer_types::builder::SidecarConfig {
        &self.config
    }
}

impl Default for BuildahSidecarBackend {
    fn default() -> Self {
        Self::new(zlayer_types::builder::SidecarConfig::default())
    }
}

fn pending(op: &'static str) -> BuildError {
    BuildError::NotSupported {
        operation: format!(
            "buildah-sidecar backend: {op} — implemented in task 3.x of the \
             buildah-sidecar plan; until then this backend is reachable via \
             --backend buildah-sidecar but is not functional"
        ),
    }
}

#[async_trait]
impl BuildBackend for BuildahSidecarBackend {
    async fn build_image(
        &self,
        _context: &Path,
        _dockerfile: &Dockerfile,
        _options: &BuildOptions,
        _event_tx: Option<std::sync::mpsc::Sender<BuildEvent>>,
    ) -> Result<BuiltImage> {
        Err(pending("build_image"))
    }

    async fn push_image(&self, _tag: &str, _auth: Option<&RegistryAuth>) -> Result<()> {
        Err(pending("push_image"))
    }

    async fn tag_image(&self, _image: &str, _new_tag: &str) -> Result<()> {
        Err(pending("tag_image"))
    }

    async fn manifest_create(&self, _name: &str) -> Result<()> {
        Err(pending("manifest_create"))
    }

    async fn manifest_add(&self, _manifest: &str, _image: &str) -> Result<()> {
        Err(pending("manifest_add"))
    }

    async fn manifest_push(&self, _name: &str, _destination: &str) -> Result<()> {
        Err(pending("manifest_push"))
    }

    async fn is_available(&self) -> bool {
        // Will probe sidecar binary on PATH / ${ZLAYER_DATA_DIR}/bin once
        // task 3.1 implements binary discovery. Until then the skeleton is
        // intentionally unavailable so detect_backend keeps choosing the
        // CLI path.
        false
    }

    fn name(&self) -> &'static str {
        Self::NAME
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_holds_config() {
        let cfg = zlayer_types::builder::SidecarConfig {
            addr: Some("127.0.0.1:1234".into()),
            tls_dir: None,
            idle_secs: 99,
        };
        let backend = BuildahSidecarBackend::new(cfg.clone());
        assert_eq!(backend.config(), &cfg);
        assert_eq!(backend.name(), "buildah-sidecar");
    }

    #[tokio::test]
    async fn default_is_unavailable_until_stage_3() {
        let backend = BuildahSidecarBackend::default();
        assert!(!backend.is_available().await);
    }

    #[test]
    fn proto_types_compile() {
        // Smoke: the generated module is wired and visible.
        let req = proto::BuildRequest::default();
        assert!(req.context_dir.is_empty());
        let _client_module_exists: Option<
            proto::build_service_client::BuildServiceClient<tonic::transport::Channel>,
        > = None;
    }
}
