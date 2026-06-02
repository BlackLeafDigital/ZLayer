//! `ZLayer` Agent - Container Runtime
//!
//! Manages container lifecycle, health checking, init actions, and proxy integration.

pub mod auth;
pub mod autoscale_controller;
pub mod bundle;
pub mod capability;
pub mod cdi;
pub mod cgroups_stats;
pub mod container_supervisor;
pub mod cron_scheduler;
pub mod dependency;
pub mod env;
pub mod error;
pub mod gpu_detector;
pub mod gpu_metrics;
pub mod gpu_sharing;
pub mod health;
pub mod init;
pub mod job;
pub mod metrics_providers;
pub mod netlink;
pub mod network_state;
pub mod overlay_manager;
pub mod proxy_manager;
pub mod runtime;
pub mod runtimes;
pub mod service;
pub mod stabilization;
pub mod storage_manager;
pub mod worker_client;

#[cfg(target_os = "windows")]
pub mod windows;

pub use autoscale_controller::{has_adaptive_scaling, AutoscaleController};
pub use bundle::*;
pub use container_supervisor::{
    ContainerSupervisor, SupervisedContainer, SupervisedState, SupervisorConfig, SupervisorEvent,
};
pub use cron_scheduler::{CronJobInfo, CronScheduler};
pub use dependency::{
    DependencyConditionChecker, DependencyError, DependencyGraph, DependencyNode, DependencyWaiter,
    WaitResult,
};
pub use env::{
    resolve_env_value, resolve_env_vars, resolve_env_with_secrets, EnvResolutionError, ResolvedEnv,
};
pub use error::*;
pub use gpu_detector::{detect_gpus, GpuInfo};
pub use health::*;
pub use init::{BackoffConfig, InitOrchestrator};
pub use job::{
    JobExecution, JobExecutionId, JobExecutor, JobExecutorConfig, JobStatus, JobTrigger,
};
pub use metrics_providers::{RuntimeStatsProvider, ServiceManagerContainerProvider};
pub use overlay_manager::{make_interface_name, OverlayManager};
pub use proxy_manager::{ProxyManager, ProxyManagerConfig};
pub use runtime::*;
pub use runtimes::{create_runtime_for_image, detect_image_artifact_type};

// Youki runtime types are only available on Linux with the `youki-runtime` feature.
#[cfg(all(target_os = "linux", feature = "youki-runtime"))]
pub use runtimes::{YoukiConfig, YoukiRuntime};

#[cfg(feature = "docker")]
pub use runtimes::DockerRuntime;

#[cfg(feature = "wasm")]
pub use runtimes::{WasmConfig, WasmRuntime};

#[cfg(target_os = "macos")]
pub use runtimes::macos_sandbox::SandboxRuntime;
#[cfg(target_os = "macos")]
pub use runtimes::macos_vm::VmRuntime;

pub use service::*;
pub use stabilization::{
    wait_for_stabilization, ServiceHealthSummary, StabilizationOutcome, StabilizationResult,
};
pub use storage_manager::{StorageError, StorageManager, VolumeInfo};
pub use worker_client::{
    WorkerClientError, WorkerClientImpl, WorkerIdentity, WorkerStatusProvider,
};

#[cfg(target_os = "macos")]
use std::path::PathBuf;
use std::sync::Arc;

/// Configuration for macOS sandbox-based container runtime
///
/// Uses Apple's sandbox framework (sandbox_init/sandbox-exec) to provide
/// process isolation on macOS. This is the preferred runtime on macOS
/// when Docker is not available or not desired.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone)]
pub struct MacSandboxConfig {
    /// Directory for container data and rootfs
    pub data_dir: PathBuf,
    /// Directory for container logs
    pub log_dir: PathBuf,
    /// Whether to enable GPU access (Metal/MPS) for containers
    pub gpu_access: bool,
}

#[cfg(target_os = "macos")]
impl Default for MacSandboxConfig {
    fn default() -> Self {
        let dirs = zlayer_paths::ZLayerDirs::system_default();
        Self {
            data_dir: dirs.data_dir().to_path_buf(),
            log_dir: dirs.logs(),
            gpu_access: true,
        }
    }
}

/// Configuration for selecting and configuring a container runtime
#[derive(Debug, Clone, Default)]
pub enum RuntimeConfig {
    /// Automatically select the best available runtime
    ///
    /// Selection logic:
    /// - On Linux: Uses bundled libcontainer runtime (no external binary needed), falls back to Docker
    /// - On macOS: Uses sandbox runtime if available, falls back to Docker
    /// - On Windows: Use Docker directly
    /// - If no runtime can be initialized, returns an error
    #[default]
    Auto,
    /// Use the mock runtime for testing and development
    Mock,
    /// Use youki/libcontainer as the container runtime (Linux only, requires the `youki-runtime` feature)
    #[cfg(all(target_os = "linux", feature = "youki-runtime"))]
    Youki(YoukiConfig),
    /// Use Docker daemon as the container runtime (cross-platform)
    #[cfg(feature = "docker")]
    Docker,
    /// Use WebAssembly runtime with wasmtime for WASM workloads
    #[cfg(feature = "wasm")]
    Wasm(WasmConfig),
    /// Use macOS sandbox-based container runtime
    #[cfg(target_os = "macos")]
    MacSandbox(MacSandboxConfig),
    /// Use macOS Virtualization.framework for full VM isolation
    #[cfg(target_os = "macos")]
    MacVm,
    /// WSL2 backend (deprecated).
    ///
    /// Preserved for one release for back-compat with existing `runtime: wsl2`
    /// configs. No real WSL2-specific backend was ever shipped — this variant
    /// was a stub that suggested using Docker Desktop with the WSL2 backend.
    #[cfg(target_os = "windows")]
    #[deprecated(
        note = "Wsl2 is deprecated in favor of Hcs (native Windows containers via the \
                Host Compute Service). This variant is preserved for one release and \
                currently aliases to Hcs with a default config at dispatch time."
    )]
    Wsl2,
    /// Native Windows container runtime via the Host Compute Service (HCS).
    ///
    /// Windows-only. Drives containers directly against the Windows HCS API
    /// (see [`crate::runtimes::hcs`]) without requiring Docker Desktop or a
    /// WSL2 VM.
    #[cfg(target_os = "windows")]
    Hcs(crate::runtimes::hcs::HcsConfig),
}

/// Check if Docker daemon is available and responsive
///
/// This function attempts to connect to the Docker daemon using
/// platform-specific defaults and pings it to verify connectivity.
///
/// # Returns
/// `true` if Docker is available, `false` otherwise
#[cfg(feature = "docker")]
pub async fn is_docker_available() -> bool {
    use bollard::Docker;

    match Docker::connect_with_local_defaults() {
        Ok(docker) => match docker.ping().await {
            Ok(_) => {
                tracing::debug!("Docker daemon is available");
                true
            }
            Err(e) => {
                tracing::debug!(error = %e, "Docker daemon ping failed");
                false
            }
        },
        Err(e) => {
            tracing::debug!(error = %e, "Failed to connect to Docker daemon");
            false
        }
    }
}

/// Check if Docker daemon is available (stub when docker feature is disabled)
#[cfg(not(feature = "docker"))]
#[allow(clippy::unused_async)]
pub async fn is_docker_available() -> bool {
    false
}

/// Check if the WASM runtime is available (compiled in)
///
/// Returns `true` if the `wasm` feature is enabled and the wasmtime
/// runtime is compiled into this binary.
///
/// # Example
///
/// ```
/// use zlayer_agent::is_wasm_available;
///
/// if is_wasm_available() {
///     println!("WASM runtime is available");
/// } else {
///     println!("WASM runtime is not compiled in");
/// }
/// ```
#[cfg(feature = "wasm")]
#[must_use]
pub fn is_wasm_available() -> bool {
    true
}

/// Check if the WASM runtime is available (stub when wasm feature is disabled)
#[cfg(not(feature = "wasm"))]
#[must_use]
pub fn is_wasm_available() -> bool {
    false
}

/// Create a runtime based on the provided configuration
///
/// # Arguments
/// * `config` - The runtime configuration specifying which runtime to use
///
/// # Returns
/// An `Arc<dyn Runtime + Send + Sync>` that can be used with `ServiceManager`
///
/// # Errors
/// Returns `AgentError` if the runtime cannot be initialized (e.g., failed to create
/// required directories, no runtime available for Auto mode)
///
/// # Runtime Selection for Auto Mode
///
/// When `RuntimeConfig::Auto` is specified:
/// - **Linux**: Uses bundled libcontainer runtime (no external binary needed), falls back to Docker
/// - **macOS**: Uses sandbox runtime (native Metal/MPS), falls back to VM runtime (libkrun), then Docker
/// - **Windows**: Uses Docker directly
/// - If no runtime can be initialized, returns an error
///
/// # Example
/// ```no_run
/// use zlayer_agent::{RuntimeConfig, create_runtime};
///
/// # async fn example() -> Result<(), zlayer_agent::AgentError> {
/// let runtime = create_runtime(RuntimeConfig::Auto, None).await?;
/// # Ok(())
/// # }
/// ```
pub async fn create_runtime(
    config: RuntimeConfig,
    auth_ctx: Option<ContainerAuthContext>,
) -> Result<Arc<dyn Runtime + Send + Sync>> {
    match config {
        RuntimeConfig::Auto => create_auto_runtime(auth_ctx).await,
        RuntimeConfig::Mock => Ok(Arc::new(MockRuntime::new())),
        #[cfg(all(target_os = "linux", feature = "youki-runtime"))]
        RuntimeConfig::Youki(youki_config) => {
            let runtime = YoukiRuntime::new(youki_config, auth_ctx).await?;
            Ok(Arc::new(runtime))
        }
        #[cfg(feature = "docker")]
        RuntimeConfig::Docker => {
            let runtime = DockerRuntime::new(auth_ctx).await?;
            Ok(Arc::new(runtime))
        }
        #[cfg(feature = "wasm")]
        RuntimeConfig::Wasm(wasm_config) => {
            let runtime = WasmRuntime::new(wasm_config, auth_ctx).await?;
            Ok(Arc::new(runtime))
        }
        #[cfg(target_os = "macos")]
        RuntimeConfig::MacSandbox(config) => {
            let primary: Arc<dyn Runtime> = Arc::new(runtimes::macos_sandbox::SandboxRuntime::new(
                config,
                auth_ctx.clone(),
            )?);
            let delegate: Option<Arc<dyn Runtime>> = match runtimes::macos_vm::VmRuntime::new(
                auth_ctx,
            ) {
                Ok(rt) => {
                    tracing::info!(
                            "macOS VM (libkrun) delegate available — Linux containers will execute in a micro-VM"
                        );
                    Some(Arc::new(rt))
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "macOS VM delegate unavailable; node will only run mac-native containers"
                    );
                    None
                }
            };
            Ok(Arc::new(runtimes::composite::CompositeRuntime::new(
                primary, delegate,
            )))
        }
        #[cfg(target_os = "macos")]
        RuntimeConfig::MacVm => Ok(Arc::new(runtimes::macos_vm::VmRuntime::new(auth_ctx)?)),
        #[cfg(target_os = "windows")]
        #[allow(deprecated)]
        RuntimeConfig::Wsl2 => {
            tracing::warn!(
                "RuntimeConfig::Wsl2 is deprecated; treating as RuntimeConfig::Hcs with default config"
            );
            Box::pin(create_runtime(
                RuntimeConfig::Hcs(crate::runtimes::hcs::HcsConfig::default()),
                auth_ctx,
            ))
            .await
        }
        #[cfg(target_os = "windows")]
        RuntimeConfig::Hcs(hcs_config) => {
            let primary: Arc<dyn Runtime> =
                Arc::new(crate::runtimes::hcs::HcsRuntime::new(hcs_config).await?);

            #[cfg(feature = "wsl")]
            let delegate: Option<Arc<dyn Runtime>> =
                match runtimes::wsl2_delegate::Wsl2DelegateRuntime::try_new().await {
                    Ok(Some(rt)) => {
                        tracing::info!(
                            "WSL2 delegate runtime available — Linux containers will execute inside the zlayer distro"
                        );
                        Some(Arc::new(rt))
                    }
                    Ok(None) => {
                        tracing::info!(
                            "WSL2 not available; node will only run Windows-image containers"
                        );
                        None
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "WSL2 delegate setup failed; node will only run Windows-image containers"
                        );
                        None
                    }
                };
            #[cfg(not(feature = "wsl"))]
            let delegate: Option<Arc<dyn Runtime>> = None;

            Ok(Arc::new(runtimes::composite::CompositeRuntime::new(
                primary, delegate,
            )))
        }
    }
}

/// Automatically select and create the best available runtime
///
/// Selection logic:
/// - On Linux: Uses bundled libcontainer runtime directly (no external binary needed), falls back to Docker
/// - On macOS: `SandboxRuntime` (native Metal/MPS) → `VmRuntime` (libkrun Linux compat with GPU) → Docker
/// - On Windows: Use Docker directly
/// - Returns an error if no runtime can be initialized
#[cfg_attr(
    not(all(target_os = "linux", feature = "youki-runtime")),
    allow(clippy::unused_async)
)]
#[cfg_attr(
    not(any(
        all(target_os = "linux", feature = "youki-runtime"),
        target_os = "macos",
        feature = "docker"
    )),
    allow(unused_variables)
)]
#[allow(clippy::too_many_lines)]
async fn create_auto_runtime(
    auth_ctx: Option<ContainerAuthContext>,
) -> Result<Arc<dyn Runtime + Send + Sync>> {
    tracing::info!("Auto-selecting container runtime");

    // On Linux, use bundled libcontainer runtime (no daemon overhead, no external binary needed)
    #[cfg(all(target_os = "linux", feature = "youki-runtime"))]
    {
        match YoukiRuntime::new(YoukiConfig::default(), auth_ctx.clone()).await {
            Ok(runtime) => {
                tracing::info!("Using bundled libcontainer runtime (Linux-native, no daemon)");
                return Ok(Arc::new(runtime));
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to initialize libcontainer runtime, trying Docker");
            }
        }
    }

    // On macOS, build a composite runtime:
    //   primary  = SandboxRuntime (native Metal/MPS), when available
    //   delegate = VmRuntime (libkrun Linux compat), when available
    // If at least the primary is available, return the composite. Otherwise
    // (e.g. sandbox init failed), fall through to Docker.
    #[cfg(target_os = "macos")]
    {
        let primary: Option<Arc<dyn Runtime>> = match runtimes::macos_sandbox::SandboxRuntime::new(
            MacSandboxConfig::default(),
            auth_ctx.clone(),
        ) {
            Ok(rt) => Some(Arc::new(rt)),
            Err(e) => {
                tracing::warn!("macOS sandbox runtime unavailable: {e}");
                None
            }
        };
        let delegate: Option<Arc<dyn Runtime>> = match runtimes::macos_vm::VmRuntime::new(
            auth_ctx.clone(),
        ) {
            Ok(rt) => {
                tracing::info!(
                        "macOS VM (libkrun) delegate available — Linux containers will execute in a micro-VM"
                    );
                Some(Arc::new(rt))
            }
            Err(e) => {
                tracing::warn!("macOS VM runtime (libkrun) unavailable: {e}");
                None
            }
        };

        if let Some(p) = primary {
            return Ok(Arc::new(runtimes::composite::CompositeRuntime::new(
                p, delegate,
            )));
        }
        // If sandbox failed but VM succeeded, use the VM runtime on its own —
        // it's still the best available native macOS path before falling back
        // to Docker.
        if let Some(d) = delegate {
            return Ok(d);
        }
    }

    // On Windows, build a composite runtime:
    //   primary  = HcsRuntime (native Windows containers), when available
    //   delegate = Wsl2DelegateRuntime (Linux containers via WSL2), when available
    // If the primary is available, return the composite. Otherwise fall
    // through to Docker.
    #[cfg(target_os = "windows")]
    {
        let primary: Option<Arc<dyn Runtime>> =
            match crate::runtimes::hcs::HcsRuntime::new(crate::runtimes::hcs::HcsConfig::default())
                .await
            {
                Ok(rt) => {
                    tracing::info!(
                        "Using native Windows HCS runtime (no Docker Desktop / WSL2 required)"
                    );
                    Some(Arc::new(rt))
                }
                Err(e) => {
                    tracing::warn!(error = %e, "HCS runtime unavailable, falling back to Docker");
                    None
                }
            };

        #[cfg(feature = "wsl")]
        let delegate: Option<Arc<dyn Runtime>> =
            match runtimes::wsl2_delegate::Wsl2DelegateRuntime::try_new().await {
                Ok(Some(rt)) => {
                    tracing::info!(
                        "WSL2 delegate runtime available — Linux containers will execute inside the zlayer distro"
                    );
                    Some(Arc::new(rt))
                }
                Ok(None) => {
                    tracing::info!(
                        "WSL2 not available; node will only run Windows-image containers"
                    );
                    None
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "WSL2 delegate setup failed; node will only run Windows-image containers"
                    );
                    None
                }
            };
        #[cfg(not(feature = "wsl"))]
        let delegate: Option<Arc<dyn Runtime>> = None;

        if let Some(p) = primary {
            return Ok(Arc::new(runtimes::composite::CompositeRuntime::new(
                p, delegate,
            )));
        }
    }

    // On non-Linux or if libcontainer failed, try Docker
    #[cfg(feature = "docker")]
    {
        if is_docker_available().await {
            tracing::info!("Selected Docker runtime");
            let runtime = DockerRuntime::new(auth_ctx).await?;
            return Ok(Arc::new(runtime));
        }
        tracing::debug!("Docker daemon not available");
    }

    // No runtime available
    #[cfg(all(target_os = "linux", feature = "docker"))]
    {
        Err(AgentError::Configuration(
            "Bundled libcontainer runtime failed to initialize and Docker daemon is not available."
                .to_string(),
        ))
    }

    #[cfg(all(target_os = "linux", not(feature = "docker")))]
    {
        Err(AgentError::Configuration(
            "Bundled libcontainer runtime failed to initialize. Enable the 'docker' feature for an alternative."
                .to_string(),
        ))
    }

    #[cfg(all(not(target_os = "linux"), feature = "docker"))]
    {
        Err(AgentError::Configuration(
            "No container runtime available. Start the Docker daemon.".to_string(),
        ))
    }

    #[cfg(all(not(target_os = "linux"), not(feature = "docker")))]
    {
        Err(AgentError::Configuration(
            "No container runtime available. Enable the 'docker' feature and start the Docker daemon.".to_string(),
        ))
    }
}
