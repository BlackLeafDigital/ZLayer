//! Container runtime implementations
//!
//! This module contains implementations of the Runtime trait for different
//! container runtimes:
//!
//! - **youki**: Linux-native container runtime using libcontainer. Preferred on Linux
//!   for its performance (no daemon overhead) and tight integration with the kernel.
//!   Only available on Linux targets.
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
//! # Platform Support
//!
//! - The youki runtime (using libcontainer) is only available on Linux targets.
//!   On non-Linux platforms, use Docker or WASM runtimes.
//!
//! # Examples
//!
//! ```no_run
//! use zlayer_agent::{RuntimeConfig, create_runtime};
//!
//! # async fn example() -> Result<(), zlayer_agent::AgentError> {
//! // Auto-select the best runtime for this platform
//! let runtime = create_runtime(RuntimeConfig::Auto, None).await?;
//!
//! // Or explicitly choose a runtime
//! #[cfg(feature = "docker")]
//! let docker_runtime = create_runtime(RuntimeConfig::Docker, None).await?;
//!
//! #[cfg(feature = "wasm")]
//! {
//!     use zlayer_agent::WasmConfig;
//!     let wasm_runtime = create_runtime(RuntimeConfig::Wasm(WasmConfig::default()), None).await?;
//! }
//! # Ok(())
//! # }
//! ```

// Cross-platform composite runtime that dispatches per-container between a
// primary runtime and an optional delegate (e.g. a WSL2 Linux delegate on a
// Windows host). Compiled unconditionally — the delegate is optional at
// runtime, so the abstraction is useful on every target.
pub mod composite;

// Youki runtime is Linux-only as it depends on libcontainer which uses
// Linux-specific APIs (cgroups, namespaces, procfs). Gated behind the
// `youki-runtime` feature so the crate publishes without pulling libcontainer.
#[cfg(all(target_os = "linux", feature = "youki-runtime"))]
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
mod wasm_http_interfaces;

#[cfg(feature = "wasm")]
mod wasm_pipeline;

#[cfg(feature = "wasm")]
pub mod wasm_test;

#[cfg(target_os = "macos")]
pub mod macos_sandbox;

#[cfg(target_os = "macos")]
pub mod macos_vm;

// Guest-agnostic helpers shared by the macOS Apple-Virtualization runtimes
// (native-macOS guests via `macos_vz`, Linux guests via `macos_vz_linux`).
#[cfg(target_os = "macos")]
pub mod macos_vz_shared;

// Apple-Virtualization (VZ) runtime: ephemeral native-macOS guest VMs. Opt-in
// only (label `com.zlayer.isolation=vz`); coexists with the Seatbelt sandbox.
#[cfg(target_os = "macos")]
pub mod macos_vz;

// Apple-Virtualization Linux-guest runtime: boots real Linux guests on macOS
// via `Virtualization.framework` (the default Linux path on macOS). Coexists
// with the libkrun `macos_vm` runtime (reachable via `com.zlayer.isolation=vm`).
#[cfg(target_os = "macos")]
pub mod macos_vz_linux;

// Producer for VZ base-image bundles: drives a macOS `.ipsw` restore through
// `VZMacOSInstaller` to mint a Tart-style bundle (`disk.img`,
// `hardware-model.bin`, `aux.img`) that `macos_vz` can pull. Used by
// `zlayer vz build-base`.
#[cfg(target_os = "macos")]
pub mod macos_vz_build;

// HCS-backed native Windows container runtime. Compiled only on Windows
// because it depends on the `zlayer-hcs` crate and the `crate::windows`
// submodule, both of which are Windows-only.
#[cfg(target_os = "windows")]
pub mod hcs;

// WSL2 delegate runtime. Compiled only on Windows with the `wsl` feature
// enabled; shells out to `youki` inside the dedicated `zlayer` WSL2 distro
// so Linux containers can run alongside HCS-managed Windows containers via
// [`composite::CompositeRuntime`].
#[cfg(all(target_os = "windows", feature = "wsl"))]
pub mod wsl2_delegate;

#[cfg(target_os = "windows")]
pub use hcs::{HcsConfig, HcsRuntime, IsolationMode};

#[cfg(all(target_os = "linux", feature = "youki-runtime"))]
pub use youki::{YoukiConfig, YoukiRuntime};

#[cfg(target_os = "macos")]
pub use macos_sandbox::SandboxRuntime;

#[cfg(target_os = "macos")]
pub use macos_vm::VmRuntime;

#[cfg(target_os = "macos")]
pub use macos_vz::VzRuntime;

#[cfg(target_os = "macos")]
pub use macos_vz_linux::VzLinuxRuntime;

#[cfg(feature = "docker")]
pub use docker::DockerRuntime;

#[cfg(feature = "wasm")]
pub use wasm::{WasmConfig, WasmRuntime};

#[cfg(feature = "wasm")]
pub use wasm_host::{
    add_to_linker, add_to_linker_with_capabilities, configure_wasi_ctx_with_capabilities,
    DefaultHost, KvError, LogLevel, MetricsStore, ZLayerHost,
};

#[cfg(feature = "wasm")]
pub use wasm_http::{HttpRequest, HttpResponse, PoolStats, WasmHttpError, WasmHttpRuntime};

#[cfg(feature = "wasm")]
pub use wasm_pipeline::WasmPipelineRuntime;

#[cfg(feature = "wasm")]
pub use wasm_http_interfaces::{
    // Utility functions
    duration_to_ns,
    ns_to_duration,
    // Caching types
    CacheDecision,
    CacheEntry,
    // Plugin traits
    CachingPlugin,
    // Common types
    DurationNs,
    // HTTP types
    HttpMethod,
    HttpVersion,
    ImmediateResponse,
    KeyValue,
    // WebSocket types
    MessageType,
    // Middleware types
    MiddlewareAction,
    MiddlewarePlugin,
    PluginRequest,
    RedirectInfo,
    RequestMetadata,
    RouterPlugin,
    RoutingDecision,
    Timestamp,
    UpgradeDecision,
    Upstream,
    // Errors
    WasmInterfaceError,
    WebSocketMessage,
    WebSocketPlugin,
};

/// Discriminated union of WASM runtime types.
///
/// Each variant corresponds to a different WASM execution model:
/// - `Http`: Full HTTP request/response handler (wasi:http/incoming-handler)
/// - `Plugin`: General plugin with full host access (zlayer:plugin/handler)
/// - `Pipeline`: Proxy-adjacent component (transformer, auth, rate-limiter, middleware, router)
#[cfg(feature = "wasm")]
pub enum WasmRuntimeKind {
    /// HTTP request handler (wasi:http/incoming-handler)
    Http(WasmHttpRuntime),
    /// General plugin runtime (zlayer:plugin/handler)
    Plugin(WasmPluginConfig),
    /// Proxy pipeline component (transformer, auth, rate-limiter, middleware, router)
    Pipeline(WasmPipelineConfig),
}

/// Configuration for a WASM plugin runtime
#[cfg(feature = "wasm")]
#[derive(Debug, Clone)]
pub struct WasmPluginConfig {
    pub capabilities: zlayer_spec::WasmCapabilities,
    pub max_memory: Option<String>,
    pub max_fuel: u64,
    pub request_timeout: std::time::Duration,
}

/// Configuration for a WASM pipeline runtime (proxy-adjacent worlds)
#[cfg(feature = "wasm")]
#[derive(Debug, Clone)]
pub struct WasmPipelineConfig {
    pub service_type: zlayer_spec::ServiceType,
    pub capabilities: zlayer_spec::WasmCapabilities,
    pub max_memory: Option<String>,
    pub max_fuel: u64,
    pub request_timeout: std::time::Duration,
    pub min_instances: u32,
    pub max_instances: u32,
}

/// Create the appropriate WASM runtime for a given service type.
///
/// Dispatches to specialized runtimes based on the WASM world:
/// - `WasmHttp` -> [`WasmHttpRuntime`] (existing)
/// - `WasmPlugin` -> General plugin runtime (uses [`WasmPluginConfig`])
/// - Proxy-adjacent types (Transformer, Auth, `RateLimiter`, Middleware, Router)
///   -> [`WasmPipelineConfig`] (for use with [`WasmPipelineRuntime`])
///
/// # Errors
///
/// Returns an error if the service type is not a WASM variant, or if runtime
/// creation fails (e.g., wasmtime engine initialization).
///
/// # Panics
///
/// Panics if `default_wasm_capabilities()` returns `None` for a known WASM
/// service type when no explicit capabilities are provided.
#[cfg(feature = "wasm")]
pub fn create_wasm_runtime_for_service_type(
    service_type: zlayer_spec::ServiceType,
    wasm_config: &zlayer_spec::WasmConfig,
) -> Result<WasmRuntimeKind, Box<dyn std::error::Error + Send + Sync>> {
    use zlayer_spec::ServiceType;

    match service_type {
        ServiceType::WasmHttp => {
            // Convert WasmConfig to the deprecated WasmHttpConfig that the
            // HTTP runtime still uses internally
            #[allow(deprecated)]
            let http_config = zlayer_spec::WasmHttpConfig {
                min_instances: wasm_config.min_instances,
                max_instances: wasm_config.max_instances,
                idle_timeout: wasm_config.idle_timeout,
                request_timeout: wasm_config.request_timeout,
            };

            // Use capability-gated constructor if capabilities are configured
            let capabilities = wasm_config
                .capabilities
                .clone()
                .unwrap_or_else(|| service_type.default_wasm_capabilities().unwrap());
            let mut runtime = WasmHttpRuntime::new_with_capabilities(http_config, capabilities)?;
            // Apply resource limits (max_memory, max_fuel) from the spec config
            runtime.set_resource_limits(wasm_config);
            Ok(WasmRuntimeKind::Http(runtime))
        }
        ServiceType::WasmPlugin => Ok(WasmRuntimeKind::Plugin(WasmPluginConfig {
            capabilities: wasm_config
                .capabilities
                .clone()
                .unwrap_or_else(|| service_type.default_wasm_capabilities().unwrap()),
            max_memory: wasm_config.max_memory.clone(),
            max_fuel: wasm_config.max_fuel,
            request_timeout: wasm_config.request_timeout,
        })),
        ServiceType::WasmTransformer
        | ServiceType::WasmAuthenticator
        | ServiceType::WasmRateLimiter
        | ServiceType::WasmMiddleware
        | ServiceType::WasmRouter => Ok(WasmRuntimeKind::Pipeline(WasmPipelineConfig {
            service_type,
            capabilities: wasm_config
                .capabilities
                .clone()
                .unwrap_or_else(|| service_type.default_wasm_capabilities().unwrap()),
            max_memory: wasm_config.max_memory.clone(),
            max_fuel: wasm_config.max_fuel,
            request_timeout: wasm_config.request_timeout,
            min_instances: wasm_config.min_instances,
            max_instances: wasm_config.max_instances,
        })),
        _ => Err(format!("service type {service_type:?} is not a WASM type").into()),
    }
}

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
    // Resolve registry auth from ~/.docker/config.json (AuthConfig default =
    // DockerConfig) for manifest detection, so `zlayer login` creds / Docker Hub
    // auth apply instead of anonymous (and avoid Docker Hub rate-limits).
    let auth = zlayer_core::AuthResolver::new(zlayer_core::AuthConfig::default()).resolve(image);

    tracing::info!(image = %image, "detecting artifact type for runtime selection");

    // Detect artifact type from manifest
    let (artifact_type, _manifest, _digest) = registry
        .detect_artifact_type(image, &auth)
        .await
        .map_err(|e| AgentError::PullFailed {
            image: image.to_string(),
            reason: format!("failed to detect artifact type: {e}"),
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
                let runtime = WasmRuntime::new(WasmConfig::default(), None).await?;
                Ok(Arc::new(runtime))
            }

            #[cfg(not(feature = "wasm"))]
            {
                Err(AgentError::Configuration(format!(
                    "Image '{image}' is a WASM artifact (WASI {wasi_version}) but the 'wasm' feature is not enabled. \
                     Recompile zlayer-agent with --features wasm to run WASM workloads."
                )))
            }
        }
        zlayer_registry::ArtifactType::Container => {
            tracing::info!(image = %image, "detected container image");

            // Use standard auto-selection for container images
            crate::create_runtime(crate::RuntimeConfig::Auto, None).await
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
    // Resolve registry auth from ~/.docker/config.json (AuthConfig default =
    // DockerConfig) so `zlayer login` creds / Docker Hub auth apply.
    let auth = zlayer_core::AuthResolver::new(zlayer_core::AuthConfig::default()).resolve(image);

    let (artifact_type, _manifest, _digest) = registry
        .detect_artifact_type(image, &auth)
        .await
        .map_err(|e| AgentError::PullFailed {
            image: image.to_string(),
            reason: format!("failed to detect artifact type: {e}"),
        })?;

    Ok(artifact_type)
}
