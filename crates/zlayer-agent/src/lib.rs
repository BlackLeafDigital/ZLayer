//! ZLayer Agent - Container Runtime
//!
//! Manages container lifecycle, health checking, init actions, and proxy integration.

pub mod autoscale_controller;
pub mod bundle;
pub mod cgroups_stats;
pub mod container_supervisor;
pub mod cron_scheduler;
pub mod dependency;
pub mod env;
pub mod error;
pub mod gpu_detector;
pub mod health;
pub mod init;
pub mod job;
pub mod metrics_providers;
pub mod overlay_manager;
pub mod proxy_manager;
pub mod runtime;
pub mod runtimes;
pub mod service;
pub mod stabilization;
pub mod storage_manager;

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

// Youki runtime types are only available on Linux
#[cfg(target_os = "linux")]
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
        let base = std::env::var("HOME")
            .map(|h| PathBuf::from(h).join(".local/share/zlayer"))
            .unwrap_or_else(|_| PathBuf::from("/var/lib/zlayer"));
        let log_dir = base.join("logs");
        Self {
            data_dir: base,
            log_dir,
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
    /// Use youki/libcontainer as the container runtime (Linux only)
    #[cfg(target_os = "linux")]
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
pub fn is_wasm_available() -> bool {
    true
}

/// Check if the WASM runtime is available (stub when wasm feature is disabled)
#[cfg(not(feature = "wasm"))]
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
/// let runtime = create_runtime(RuntimeConfig::Auto).await?;
/// # Ok(())
/// # }
/// ```
pub async fn create_runtime(config: RuntimeConfig) -> Result<Arc<dyn Runtime + Send + Sync>> {
    match config {
        RuntimeConfig::Auto => create_auto_runtime().await,
        RuntimeConfig::Mock => Ok(Arc::new(MockRuntime::new())),
        #[cfg(target_os = "linux")]
        RuntimeConfig::Youki(youki_config) => {
            let runtime = YoukiRuntime::new(youki_config).await?;
            Ok(Arc::new(runtime))
        }
        #[cfg(feature = "docker")]
        RuntimeConfig::Docker => {
            let runtime = DockerRuntime::new().await?;
            Ok(Arc::new(runtime))
        }
        #[cfg(feature = "wasm")]
        RuntimeConfig::Wasm(wasm_config) => {
            let runtime = WasmRuntime::new(wasm_config).await?;
            Ok(Arc::new(runtime))
        }
        #[cfg(target_os = "macos")]
        RuntimeConfig::MacSandbox(config) => Ok(Arc::new(
            runtimes::macos_sandbox::SandboxRuntime::new(config)?,
        )),
        #[cfg(target_os = "macos")]
        RuntimeConfig::MacVm => Ok(Arc::new(runtimes::macos_vm::VmRuntime::new()?)),
    }
}

/// Automatically select and create the best available runtime
///
/// Selection logic:
/// - On Linux: Uses bundled libcontainer runtime directly (no external binary needed), falls back to Docker
/// - On macOS: SandboxRuntime (native Metal/MPS) → VmRuntime (libkrun Linux compat with GPU) → Docker
/// - On Windows: Use Docker directly
/// - Returns an error if no runtime can be initialized
async fn create_auto_runtime() -> Result<Arc<dyn Runtime + Send + Sync>> {
    tracing::info!("Auto-selecting container runtime");

    // On Linux, use bundled libcontainer runtime (no daemon overhead, no external binary needed)
    #[cfg(target_os = "linux")]
    {
        match YoukiRuntime::new(YoukiConfig::default()).await {
            Ok(runtime) => {
                tracing::info!("Using bundled libcontainer runtime (Linux-native, no daemon)");
                return Ok(Arc::new(runtime));
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to initialize libcontainer runtime, trying Docker");
            }
        }
    }

    // On macOS, try sandbox → VM → Docker
    #[cfg(target_os = "macos")]
    {
        // Try sandbox first (native Metal/MPS performance)
        match runtimes::macos_sandbox::SandboxRuntime::new(MacSandboxConfig::default()) {
            Ok(rt) => return Ok(Arc::new(rt)),
            Err(e) => tracing::warn!("macOS sandbox runtime unavailable: {e}"),
        }
        // Fall back to VM runtime (Linux container compat with GPU forwarding)
        match runtimes::macos_vm::VmRuntime::new() {
            Ok(rt) => return Ok(Arc::new(rt)),
            Err(e) => tracing::warn!("macOS VM runtime (libkrun) unavailable: {e}"),
        }
    }

    // On non-Linux or if libcontainer failed, try Docker
    #[cfg(feature = "docker")]
    {
        if is_docker_available().await {
            tracing::info!("Selected Docker runtime");
            let runtime = DockerRuntime::new().await?;
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
