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
use crate::error::Result;
use crate::tui::BuildEvent;

/// Generated tonic + prost bindings for `proto/buildah_sidecar.proto`.
pub mod proto {
    #![allow(clippy::all, missing_docs, clippy::pedantic, clippy::nursery)]
    tonic::include_proto!("zlayer.buildah_sidecar.v1");
}

pub mod build;
pub mod discover;
pub mod lifecycle;
pub mod ops;
pub mod tls;

pub use lifecycle::{LiveSidecar, SidecarLifecycle};
pub use tls::{ensure_tls_material, TlsMaterial};

/// gRPC-client backend for the `zlayer-buildd` sidecar.
///
/// Construction does not connect or spawn — that happens lazily on the
/// first call to [`BuildahSidecarBackend::lifecycle`] (or via
/// `is_available`, which probes spawn-and-handshake). The backend:
///
/// - holds the resolved `SidecarConfig` (transport address, TLS dir,
///   idle timeout),
/// - owns the [`SidecarLifecycle`] manager that spawns / dials the
///   sidecar and caches the gRPC channel,
/// - returns `BuildError::NotSupported` from every trait method until
///   tasks 3.3 / 3.4 wire the build flow through the gRPC channel.
#[derive(Debug, Clone)]
pub struct BuildahSidecarBackend {
    config: Arc<zlayer_types::builder::SidecarConfig>,
    lifecycle: Arc<SidecarLifecycle>,
}

impl BuildahSidecarBackend {
    /// Stable trait-level name. Mirrors `BuildBackend::name`.
    pub const NAME: &'static str = "buildah-sidecar";

    /// Create a new sidecar backend with the supplied configuration.
    #[must_use]
    pub fn new(config: zlayer_types::builder::SidecarConfig) -> Self {
        let config = Arc::new(config);
        let lifecycle = Arc::new(SidecarLifecycle::new(Arc::clone(&config)));
        Self { config, lifecycle }
    }

    /// Borrow the underlying configuration.
    #[must_use]
    pub fn config(&self) -> &zlayer_types::builder::SidecarConfig {
        &self.config
    }

    /// Access the lifecycle manager. Used by the RPC wiring landing in
    /// tasks 3.3 / 3.4 to acquire a live `BuildServiceClient`.
    #[must_use]
    pub fn lifecycle(&self) -> &Arc<SidecarLifecycle> {
        &self.lifecycle
    }
}

impl Default for BuildahSidecarBackend {
    fn default() -> Self {
        Self::new(zlayer_types::builder::SidecarConfig::default())
    }
}

#[async_trait]
impl BuildBackend for BuildahSidecarBackend {
    async fn build_image(
        &self,
        context: &Path,
        dockerfile: &Dockerfile,
        options: &BuildOptions,
        event_tx: Option<std::sync::mpsc::Sender<BuildEvent>>,
    ) -> Result<BuiltImage> {
        self.build_image_impl(context, dockerfile, options, event_tx)
            .await
    }

    async fn push_image(&self, tag: &str, auth: Option<&RegistryAuth>) -> Result<()> {
        self.push_image_impl(tag, auth).await
    }

    async fn tag_image(&self, image: &str, new_tag: &str) -> Result<()> {
        self.tag_image_impl(image, new_tag).await
    }

    async fn manifest_create(&self, name: &str) -> Result<()> {
        self.manifest_create_impl(name).await
    }

    async fn manifest_add(&self, manifest: &str, image: &str) -> Result<()> {
        self.manifest_add_impl(manifest, image).await
    }

    async fn manifest_push(&self, name: &str, destination: &str) -> Result<()> {
        self.manifest_push_impl(name, destination).await
    }

    async fn is_available(&self) -> bool {
        // Real probe: try to spawn (or dial-remote) the sidecar and
        // complete the mTLS handshake. Any failure (missing binary,
        // bad TLS material, handshake timeout, dial error) flips us
        // unavailable so `detect_backend` falls back to the CLI path.
        self.lifecycle.ensure().await.is_ok()
    }

    fn name(&self) -> &'static str {
        Self::NAME
    }
}

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use super::*;
    use crate::TEST_ENV_LOCK;

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

    // The env-lock guard must be held across `.await` so no other
    // test races us on env mutation; see lifecycle.rs for the
    // matching justification.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn default_is_unavailable_when_binary_missing() {
        let _g = TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Snapshot env, then strip anything that would let the
        // discovery path succeed.
        let prev_path = std::env::var_os("PATH");
        let prev_buildd_bin = std::env::var_os("ZLAYER_BUILDD_BIN");
        let prev_data_dir = std::env::var_os("ZLAYER_DATA_DIR");

        let tmp = tempfile::tempdir().unwrap();
        // SAFETY: env mutation serialized by `TEST_ENV_LOCK`.
        unsafe {
            std::env::remove_var("ZLAYER_BUILDD_BIN");
            std::env::set_var("PATH", "/nonexistent-zlayer-test-dir");
            // Point ZLAYER_DATA_DIR at a fresh tempdir so the
            // `${data}/bin/zlayer-buildd` candidate is also missing.
            std::env::set_var("ZLAYER_DATA_DIR", tmp.path());
        }

        let cfg = zlayer_types::builder::SidecarConfig {
            addr: None,
            // Keep TLS material out of the real ${data}/buildd by
            // pointing at the same tempdir.
            tls_dir: Some(tmp.path().to_path_buf()),
            idle_secs: 30,
        };
        let backend = BuildahSidecarBackend::new(cfg);
        let available = backend.is_available().await;

        // SAFETY: env mutation serialized by `TEST_ENV_LOCK`.
        unsafe {
            match prev_path {
                Some(v) => std::env::set_var("PATH", v),
                None => std::env::remove_var("PATH"),
            }
            match prev_buildd_bin {
                Some(v) => std::env::set_var("ZLAYER_BUILDD_BIN", v),
                None => std::env::remove_var("ZLAYER_BUILDD_BIN"),
            }
            match prev_data_dir {
                Some(v) => std::env::set_var("ZLAYER_DATA_DIR", v),
                None => std::env::remove_var("ZLAYER_DATA_DIR"),
            }
        }

        assert!(
            !available,
            "is_available should be false when zlayer-buildd cannot be discovered"
        );
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
