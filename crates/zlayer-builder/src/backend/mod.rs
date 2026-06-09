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
/// Thin wrapper around [`detect_backend_with_options`] that passes
/// `options = None`. Use this from call sites that have no access to a
/// [`BuildOptions`] (early-construction probes, pipeline scaffolding) and
/// therefore can only consult `ZLAYER_BACKEND` + the auto-detect matrix.
///
/// Selection matrix (host × target), when no override is set:
///
/// | Host / Target | Linux image                                            | Windows image                             |
/// |---------------|--------------------------------------------------------|-------------------------------------------|
/// | Linux         | buildah-sidecar if available, else buildah-cli         | Err — requires Windows host               |
/// | macOS         | buildah-cli (if available) else macos-sandbox          | Err — requires Windows host               |
/// | Windows       | Err — Linux peer required (WSL2 follow-up)             | HCS-backed native Windows builder (L-4)   |
///
/// # Errors
///
/// Returns an error if the host cannot build images for the requested
/// `target_os`, or if the selected backend's tooling is missing.
pub async fn detect_backend(target_os: ImageOs) -> Result<Arc<dyn BuildBackend>> {
    detect_backend_with_options(target_os, None).await
}

/// Auto-detect the best available build backend, honoring an optional
/// per-build override on [`BuildOptions::backend_override`].
///
/// Selection precedence:
///
/// 1. `BuildOptions::backend_override` (when `options` is `Some`).
/// 2. `ZLAYER_BACKEND` environment variable.
/// 3. The host × target auto-detect matrix documented on [`detect_backend`].
///
/// When the precedence falls on step 1 or 2 (an explicit operator choice),
/// the requested backend is **constructed unconditionally** — we do not
/// silently fall back to a different kind. If the requested backend is
/// later found to be unavailable, the actual build call will surface the
/// failure rather than letting auto-fallback mask the operator's intent.
///
/// # Errors
///
/// Returns [`BuildError::NotSupported`] when the requested (or auto-selected)
/// backend kind cannot be paired with `target_os` on this host, and bubbles
/// any constructor error from the chosen backend.
pub async fn detect_backend_with_options(
    target_os: ImageOs,
    options: Option<&BuildOptions>,
) -> Result<Arc<dyn BuildBackend>> {
    use zlayer_types::builder::BuilderBackendKind;

    // 1) Explicit override on BuildOptions.
    if let Some(opts) = options {
        if let Some(kind) = opts.backend_override {
            return construct_backend(kind, target_os).await;
        }
    }

    // 2) ZLAYER_BACKEND env var. Accepted values match
    //    `BuilderBackendKind::from_str` (buildah / buildah-cli /
    //    buildah-sidecar / sidecar / sandbox / hcs / etc.).
    if let Ok(env_val) = std::env::var("ZLAYER_BACKEND") {
        let parsed: BuilderBackendKind =
            env_val
                .parse()
                .map_err(|e: String| BuildError::NotSupported {
                    operation: format!("ZLAYER_BACKEND={env_val}: {e}"),
                })?;
        return construct_backend(parsed, target_os).await;
    }

    // 3) Auto-detect per host × target.
    #[cfg(target_os = "windows")]
    {
        match target_os {
            ImageOs::Linux => Err(BuildError::NotSupported {
                operation: "Linux image building on Windows hosts requires a Linux peer \
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
                // Preferred macOS path: route to a `zlayer-buildd` sidecar
                // running in a VZ-Linux container when one has been wired up
                // by the build front-end (env-configured remote addr). This
                // is the only way to run real Linux Dockerfile `RUN` steps on
                // a macOS host — native buildah can't, and the Seatbelt
                // sandbox only covers a narrow subset.
                if let Some(sidecar) = sidecar_from_env() {
                    if sidecar.is_available().await {
                        return Ok(Arc::new(sidecar));
                    }
                    tracing::warn!(
                        "ZLAYER_BUILDD_ADDR set but sidecar not reachable; \
                         falling back to buildah-cli / sandbox"
                    );
                }
                if let Ok(backend) = BuildahBackend::try_new().await {
                    Ok(Arc::new(backend))
                } else {
                    tracing::info!(
                        "Buildah not available on macOS, falling back to sandbox backend"
                    );
                    Ok(Arc::new(SandboxBackend::default()))
                }
            }
            ImageOs::Windows => Err(BuildError::NotSupported {
                operation: "building Windows images requires a Windows host — run this build \
                            on a Windows node of the ZLayer cluster"
                    .to_string(),
            }),
        }
    }

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        match target_os {
            ImageOs::Linux => {
                // Prefer the sidecar backend (Stage 3 default) when it can
                // actually spawn / dial `zlayer-buildd` and complete a TLS
                // handshake; otherwise fall back to the CLI shellout. The
                // sidecar probe is cheap when the binary is missing
                // (filesystem misses + ZLAYER_BUILDD_BIN check), so this
                // is fine to run on the hot path.
                let sidecar = BuildahSidecarBackend::default();
                if sidecar.is_available().await {
                    Ok(Arc::new(sidecar))
                } else {
                    tracing::debug!(
                        "buildah-sidecar unavailable on Linux host, falling back to buildah-cli"
                    );
                    let cli = BuildahBackend::new().await?;
                    Ok(Arc::new(cli))
                }
            }
            ImageOs::Windows => Err(BuildError::NotSupported {
                operation: "building Windows images requires a Windows host — run this build \
                            on a Windows node of the ZLayer cluster"
                    .to_string(),
            }),
        }
    }
}

/// Build a [`BuildahSidecarBackend`] from environment configuration, if the
/// macOS build front-end has wired one up.
///
/// The front-end (`zlayer build` on macOS) starts a `zlayer-buildd` in a
/// VZ-Linux container and exports the wiring through env vars:
///
/// - `ZLAYER_BUILDD_ADDR` — `host:port` to dial (required; presence is the
///   on/off switch).
/// - `ZLAYER_BUILDD_TLS_DIR` — mTLS material dir (optional; defaults to the
///   per-user `${data}/buildd`).
/// - `ZLAYER_BUILDD_CONTEXT_MOUNT` — `HOST_PREFIX:GUEST_PREFIX` so the
///   backend rewrites context paths to what the in-guest buildah sees.
///
/// Returns `None` when `ZLAYER_BUILDD_ADDR` is unset (no managed sidecar).
#[cfg(target_os = "macos")]
fn sidecar_from_env() -> Option<BuildahSidecarBackend> {
    use std::path::PathBuf;
    use zlayer_types::builder::SidecarConfig;

    let addr = std::env::var("ZLAYER_BUILDD_ADDR").ok()?;
    let tls_dir = std::env::var("ZLAYER_BUILDD_TLS_DIR")
        .ok()
        .map(PathBuf::from);
    let context_mount = std::env::var("ZLAYER_BUILDD_CONTEXT_MOUNT")
        .ok()
        .and_then(|s| {
            s.split_once(':')
                .map(|(h, g)| (PathBuf::from(h), PathBuf::from(g)))
        });

    Some(BuildahSidecarBackend::new(SidecarConfig {
        addr: Some(addr),
        tls_dir,
        context_mount,
        ..Default::default()
    }))
}

/// Construct the backend implementation for the requested
/// [`BuilderBackendKind`], validating that the kind is meaningful for the
/// requested image OS.
///
/// Compatibility matrix:
///
/// - `BuildahCli`     ↔ Linux target (Linux/macOS host only)
/// - `BuildahSidecar` ↔ Linux target (Linux host only)
/// - `Sandbox`        ↔ Linux target (macOS host only — Seatbelt builder)
/// - `Hcs`            ↔ Windows target (Windows host only)
///
/// Anything outside that matrix is rejected with
/// [`BuildError::NotSupported`] so the operator gets a clear error instead
/// of a silent fallback.
#[cfg_attr(windows, allow(clippy::needless_return))]
async fn construct_backend(
    kind: zlayer_types::builder::BuilderBackendKind,
    target_os: ImageOs,
) -> Result<Arc<dyn BuildBackend>> {
    use zlayer_types::builder::BuilderBackendKind;

    match kind {
        BuilderBackendKind::BuildahCli => {
            if target_os != ImageOs::Linux {
                return Err(BuildError::NotSupported {
                    operation: format!(
                        "buildah-cli backend can only build Linux images, requested target_os={target_os:?}"
                    ),
                });
            }
            #[cfg(target_os = "windows")]
            {
                return Err(BuildError::NotSupported {
                    operation: "buildah-cli backend is not available on Windows hosts \
                                (requires a Linux peer)"
                        .to_string(),
                });
            }
            #[cfg(not(target_os = "windows"))]
            {
                let backend = BuildahBackend::new().await?;
                Ok(Arc::new(backend))
            }
        }
        BuilderBackendKind::BuildahSidecar => {
            if target_os != ImageOs::Linux {
                return Err(BuildError::NotSupported {
                    operation: format!(
                        "buildah-sidecar backend can only build Linux images, requested target_os={target_os:?}"
                    ),
                });
            }
            // Linux hosts spawn a local sidecar; macOS hosts dial a
            // `zlayer-buildd` running in a VZ-Linux container (env-wired by
            // the `zlayer build` front-end). Windows has no sidecar path.
            #[cfg(target_os = "linux")]
            {
                Ok(Arc::new(BuildahSidecarBackend::default()))
            }
            #[cfg(target_os = "macos")]
            {
                // Honor the env-configured remote sidecar when present so an
                // explicit `--backend buildah-sidecar` / `ZLAYER_BACKEND`
                // dials the managed VZ buildd instead of trying to spawn one
                // locally (which can't run on macOS).
                Ok(Arc::new(sidecar_from_env().unwrap_or_default()))
            }
            #[cfg(all(not(target_os = "linux"), not(target_os = "macos")))]
            {
                Err(BuildError::NotSupported {
                    operation: "buildah-sidecar backend is not available on this host \
                                (zlayer-buildd runs on Linux or in a macOS VZ container)"
                        .to_string(),
                })
            }
        }
        BuilderBackendKind::Sandbox => {
            if target_os != ImageOs::Linux {
                return Err(BuildError::NotSupported {
                    operation: format!(
                        "macOS sandbox backend can only build Linux images, requested target_os={target_os:?}"
                    ),
                });
            }
            #[cfg(target_os = "macos")]
            {
                Ok(Arc::new(SandboxBackend::default()))
            }
            #[cfg(not(target_os = "macos"))]
            {
                Err(BuildError::NotSupported {
                    operation: "sandbox backend is only available on macOS hosts".to_string(),
                })
            }
        }
        BuilderBackendKind::Hcs => {
            if target_os != ImageOs::Windows {
                return Err(BuildError::NotSupported {
                    operation: format!(
                        "HCS backend can only build Windows images, requested target_os={target_os:?}"
                    ),
                });
            }
            #[cfg(target_os = "windows")]
            {
                let backend = HcsBackend::new().await?;
                Ok(Arc::new(backend))
            }
            #[cfg(not(target_os = "windows"))]
            {
                Err(BuildError::NotSupported {
                    operation: "HCS backend is only available on Windows hosts".to_string(),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zlayer_types::builder::BuilderBackendKind;

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

    /// `construct_backend` must reject (kind × `target_os`) pairs that cannot
    /// possibly succeed regardless of host. The full host-platform matrix is
    /// covered in the cfg-gated branches inside `construct_backend`; this
    /// test pins the cross-target rejections so a future refactor cannot
    /// silently let `Hcs` accept a Linux target (or vice versa).
    #[tokio::test]
    async fn construct_backend_rejects_mismatched_target_os() {
        // HCS is Windows-only.
        fn assert_not_supported(result: Result<Arc<dyn BuildBackend>>, label: &str) {
            match result {
                Ok(_) => panic!("{label}: expected NotSupported, got Ok"),
                Err(BuildError::NotSupported { .. }) => {}
                Err(other) => panic!("{label}: expected NotSupported, got: {other:?}"),
            }
        }

        // HCS is Windows-only.
        assert_not_supported(
            construct_backend(BuilderBackendKind::Hcs, ImageOs::Linux).await,
            "HCS + Linux target",
        );

        // Buildah CLI is Linux-image only.
        assert_not_supported(
            construct_backend(BuilderBackendKind::BuildahCli, ImageOs::Windows).await,
            "BuildahCli + Windows target",
        );

        // Buildah sidecar is Linux-image only.
        assert_not_supported(
            construct_backend(BuilderBackendKind::BuildahSidecar, ImageOs::Windows).await,
            "BuildahSidecar + Windows target",
        );

        // Sandbox is Linux-image only (and macOS-host only).
        assert_not_supported(
            construct_backend(BuilderBackendKind::Sandbox, ImageOs::Windows).await,
            "Sandbox + Windows target",
        );
    }

    /// Smoke test for the Linux × Linux precedence: with `zlayer-buildd`
    /// missing from PATH, `detect_backend(ImageOs::Linux)` must fall back to
    /// the CLI backend (`BuildahBackend`). Gated behind `#[ignore]` because
    /// it mutates process-wide environment variables.
    ///
    /// Run manually with:
    ///   `cargo test -p zlayer-builder --lib backend::tests::detect_backend_falls_back_to_cli_when_sidecar_missing -- --ignored --nocapture`
    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "mutates PATH / ZLAYER_BUILDD_BIN / ZLAYER_DATA_DIR; serialize manually"]
    #[allow(unsafe_code, clippy::await_holding_lock)]
    async fn detect_backend_falls_back_to_cli_when_sidecar_missing() {
        let _g = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let prev_path = std::env::var_os("PATH");
        let prev_buildd = std::env::var_os("ZLAYER_BUILDD_BIN");
        let prev_data = std::env::var_os("ZLAYER_DATA_DIR");
        let prev_backend = std::env::var_os("ZLAYER_BACKEND");

        let tmp = tempfile::tempdir().expect("tempdir");

        // SAFETY: env mutation is serialized by `TEST_ENV_LOCK`.
        unsafe {
            std::env::remove_var("ZLAYER_BUILDD_BIN");
            std::env::remove_var("ZLAYER_BACKEND");
            std::env::set_var("PATH", tmp.path());
            std::env::set_var("ZLAYER_DATA_DIR", tmp.path());
        }

        let result = detect_backend(ImageOs::Linux).await;

        // SAFETY: env mutation is serialized by `TEST_ENV_LOCK`.
        unsafe {
            match prev_path {
                Some(v) => std::env::set_var("PATH", v),
                None => std::env::remove_var("PATH"),
            }
            match prev_buildd {
                Some(v) => std::env::set_var("ZLAYER_BUILDD_BIN", v),
                None => std::env::remove_var("ZLAYER_BUILDD_BIN"),
            }
            match prev_data {
                Some(v) => std::env::set_var("ZLAYER_DATA_DIR", v),
                None => std::env::remove_var("ZLAYER_DATA_DIR"),
            }
            match prev_backend {
                Some(v) => std::env::set_var("ZLAYER_BACKEND", v),
                None => std::env::remove_var("ZLAYER_BACKEND"),
            }
        }

        // The CLI fallback requires `buildah` on PATH to construct; if the
        // test box has no buildah at all, both backends are unavailable and
        // we can only assert the type didn't pick the sidecar.
        match result {
            Ok(backend) => assert_eq!(
                backend.name(),
                "buildah",
                "expected CLI fallback ('buildah'), got: {}",
                backend.name(),
            ),
            Err(e) => {
                // Acceptable: no buildah binary on this box either.
                eprintln!("detect_backend returned err (no buildah on PATH?): {e}");
            }
        }
    }
}
