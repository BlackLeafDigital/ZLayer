//! Container runtime implementations
//!
//! This module contains implementations of the Runtime trait for different
//! container runtimes:
//!
//! - **youki**: Linux-native container runtime using libcontainer. Preferred on Linux
//!   for its performance (no daemon overhead) and tight integration with the kernel.
//! - **docker**: Cross-platform runtime using Docker daemon. Works on Linux, Windows,
//!   and macOS. Requires Docker Desktop or Docker daemon to be running.
//! - **wasm**: WebAssembly runtime using wasmtime for executing WASM workloads with
//!   WASI support. Lightweight alternative to containers for compatible workloads.
//!
//! # Runtime Selection
//!
//! Use [`RuntimeConfig::Auto`](crate::RuntimeConfig::Auto) to automatically select
//! the best available runtime for container images:
//!
//! - On Linux: Prefers youki if available, falls back to Docker
//! - On Windows/macOS: Uses Docker (youki is Linux-only)
//!
//! For WASM artifacts, use [`create_runtime_for_image`] to automatically detect
//! the artifact type and create the appropriate runtime:
//!
//! ```no_run
//! use zlayer_agent::runtimes::create_runtime_for_image;
//! use zlayer_registry::{BlobCache, ImagePuller};
//! use std::sync::Arc;
//!
//! # async fn example() -> Result<(), zlayer_agent::AgentError> {
//! let cache = BlobCache::open("/tmp/blobs.redb").unwrap();
//! let registry = Arc::new(ImagePuller::new(cache));
//!
//! // Automatically detect and create the right runtime
//! let runtime = create_runtime_for_image(
//!     "ghcr.io/myorg/myimage:latest",
//!     registry,
//! ).await?;
//! # Ok(())
//! # }
//! ```
//!
//! # WASM Runtime
//!
//! The WASM runtime provides an alternative execution model for WebAssembly workloads.
//! WASM artifacts are detected by examining the OCI manifest for:
//!
//! 1. **Artifact type** (OCI 1.1+): `application/vnd.wasm.component.v1+wasm` or
//!    `application/vnd.wasm.module.v1+wasm`
//! 2. **Config media type**: `application/vnd.wasm.config.v1+json`
//! 3. **Layer media types**: `application/vnd.wasm.content.layer.v1.wasm` or `application/wasm`
//!
//! ## WASI Versions
//!
//! - **WASI Preview 1 (wasip1)**: Core module format with basic syscall interface
//! - **WASI Preview 2 (wasip2)**: Component model format (detected but not yet fully supported)
//!
//! ## Limitations
//!
//! The WASM runtime has some inherent limitations compared to container runtimes:
//!
//! - No `exec` support (WASM modules are single-process)
//! - No cgroup statistics (WASM runs in-process without kernel isolation)
//! - Limited filesystem access (environment variables and args only currently)
//!
//! # Feature Flags
//!
//! - `docker`: Enables the Docker runtime (uses bollard crate)
//! - `wasm`: Enables the WebAssembly runtime (uses wasmtime crate)
//!
//! # Examples
//!
//! ```no_run
//! use zlayer_agent::{RuntimeConfig, create_runtime};
//!
//! # async fn example() -> Result<(), zlayer_agent::AgentError> {
//! // Auto-select the best runtime for this platform
//! let runtime = create_runtime(RuntimeConfig::Auto).await?;
//!
//! // Or explicitly choose a runtime
//! #[cfg(feature = "docker")]
//! let docker_runtime = create_runtime(RuntimeConfig::Docker).await?;
//!
//! #[cfg(feature = "wasm")]
//! {
//!     use zlayer_agent::WasmConfig;
//!     let wasm_runtime = create_runtime(RuntimeConfig::Wasm(WasmConfig::default())).await?;
//! }
//! # Ok(())
//! # }
//! ```

mod youki;

#[cfg(feature = "docker")]
mod docker;

#[cfg(feature = "wasm")]
mod wasm;

#[cfg(feature = "wasm")]
mod wasm_host;

#[cfg(feature = "wasm")]
mod wasm_http;

#[cfg(feature = "wasm")]
pub mod wasm_test;

pub use youki::{YoukiConfig, YoukiRuntime};

#[cfg(feature = "docker")]
pub use docker::DockerRuntime;

#[cfg(feature = "wasm")]
pub use wasm::{WasmConfig, WasmRuntime};

#[cfg(feature = "wasm")]
pub use wasm_host::{add_to_linker, DefaultHost, KvError, LogLevel, MetricsStore, ZLayerHost};

#[cfg(feature = "wasm")]
pub use wasm_http::{HttpRequest, HttpResponse, PoolStats, WasmHttpError, WasmHttpRuntime};

use crate::error::{AgentError, Result};
use crate::runtime::Runtime;
use std::sync::Arc;
use zlayer_registry::ImagePuller;

/// Detect the artifact type of an image and create the appropriate runtime
///
/// This function pulls the image manifest from the registry to determine whether
/// the image is a traditional container image or a WASM artifact, then creates
/// the appropriate runtime to execute it.
///
/// # Detection Logic
///
/// 1. Pull the manifest from the registry
/// 2. Examine the manifest for WASM indicators (artifact type, config media type, layer types)
/// 3. If WASM: Return a [`WasmRuntime`] (requires `wasm` feature)
/// 4. If container: Return the best available container runtime via [`crate::create_runtime`]
///
/// # Arguments
///
/// * `image` - Image reference (e.g., "ghcr.io/myorg/mymodule:v1.0" or "docker.io/library/nginx:latest")
/// * `registry` - Registry client for pulling manifests. Can be shared across calls.
///
/// # Returns
///
/// Returns an `Arc<dyn Runtime + Send + Sync>` suitable for executing the image.
///
/// # Errors
///
/// - Returns [`AgentError::PullFailed`] if the manifest cannot be fetched
/// - Returns [`AgentError::Configuration`] if a WASM image is detected but the `wasm` feature
///   is not enabled
/// - Returns [`AgentError::Configuration`] if no suitable container runtime is available
///
/// # Examples
///
/// ```no_run
/// use zlayer_agent::runtimes::create_runtime_for_image;
/// use zlayer_registry::{BlobCache, ImagePuller};
/// use std::sync::Arc;
///
/// # async fn example() -> Result<(), zlayer_agent::AgentError> {
/// // Create a registry client
/// let cache = BlobCache::open("/tmp/blobs.redb").unwrap();
/// let registry = Arc::new(ImagePuller::new(cache));
///
/// // Detect and create runtime for a container image
/// let nginx_runtime = create_runtime_for_image(
///     "docker.io/library/nginx:latest",
///     Arc::clone(&registry),
/// ).await?;
///
/// // Detect and create runtime for a WASM image
/// #[cfg(feature = "wasm")]
/// {
///     let wasm_runtime = create_runtime_for_image(
///         "ghcr.io/example/wasm-module:v1.0",
///         registry,
///     ).await?;
/// }
/// # Ok(())
/// # }
/// ```
pub async fn create_runtime_for_image(
    image: &str,
    registry: Arc<ImagePuller>,
) -> Result<Arc<dyn Runtime + Send + Sync>> {
    // Use anonymous auth for manifest detection
    // In production, the caller should configure auth on the registry client
    let auth = zlayer_registry::RegistryAuth::Anonymous;

    tracing::info!(image = %image, "detecting artifact type for runtime selection");

    // Detect artifact type from manifest
    let (artifact_type, _manifest, _digest) = registry
        .detect_artifact_type(image, &auth)
        .await
        .map_err(|e| AgentError::PullFailed {
            image: image.to_string(),
            reason: format!("failed to detect artifact type: {}", e),
        })?;

    match artifact_type {
        zlayer_registry::ArtifactType::Wasm { wasi_version } => {
            tracing::info!(
                image = %image,
                wasi_version = %wasi_version,
                "detected WASM artifact"
            );

            #[cfg(feature = "wasm")]
            {
                // Create WASM runtime
                let runtime = WasmRuntime::new(WasmConfig::default()).await?;
                Ok(Arc::new(runtime))
            }

            #[cfg(not(feature = "wasm"))]
            {
                Err(AgentError::Configuration(format!(
                    "Image '{}' is a WASM artifact (WASI {}) but the 'wasm' feature is not enabled. \
                     Recompile zlayer-agent with --features wasm to run WASM workloads.",
                    image, wasi_version
                )))
            }
        }
        zlayer_registry::ArtifactType::Container => {
            tracing::info!(image = %image, "detected container image");

            // Use standard auto-selection for container images
            crate::create_runtime(crate::RuntimeConfig::Auto).await
        }
    }
}

/// Detect the artifact type of an image without creating a runtime
///
/// This is useful for pre-flight checks or when you need to know the artifact
/// type before deciding how to handle an image.
///
/// # Arguments
///
/// * `image` - Image reference (e.g., "ghcr.io/myorg/mymodule:v1.0")
/// * `registry` - Registry client for pulling manifests
///
/// # Returns
///
/// Returns the detected [`ArtifactType`](zlayer_registry::ArtifactType).
///
/// # Errors
///
/// Returns [`AgentError::PullFailed`] if the manifest cannot be fetched.
pub async fn detect_image_artifact_type(
    image: &str,
    registry: Arc<ImagePuller>,
) -> Result<zlayer_registry::ArtifactType> {
    let auth = zlayer_registry::RegistryAuth::Anonymous;

    let (artifact_type, _manifest, _digest) = registry
        .detect_artifact_type(image, &auth)
        .await
        .map_err(|e| AgentError::PullFailed {
            image: image.to_string(),
            reason: format!("failed to detect artifact type: {}", e),
        })?;

    Ok(artifact_type)
}
