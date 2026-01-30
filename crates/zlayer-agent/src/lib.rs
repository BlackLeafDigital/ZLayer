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
pub mod health;
pub mod init;
pub mod job;
pub mod metrics_providers;
pub mod overlay_manager;
pub mod proxy_manager;
pub mod runtime;
pub mod runtimes;
pub mod service;
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
pub use env::{resolve_env_value, resolve_env_vars, EnvResolutionError, ResolvedEnv};
pub use error::*;
pub use health::*;
pub use init::{BackoffConfig, InitOrchestrator};
pub use job::{
    JobExecution, JobExecutionId, JobExecutor, JobExecutorConfig, JobStatus, JobTrigger,
};
pub use metrics_providers::{RuntimeStatsProvider, ServiceManagerContainerProvider};
pub use overlay_manager::OverlayManager;
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

pub use service::*;
pub use storage_manager::{StorageError, StorageManager, VolumeInfo};

use std::sync::Arc;

/// Configuration for selecting and configuring a container runtime
#[derive(Debug, Clone)]
pub enum RuntimeConfig {
    /// Automatically select the best available runtime
    ///
    /// Selection logic:
    /// - On Linux: Try youki first (if binary available), fall back to Docker
    /// - On Windows/macOS: Use Docker directly
    /// - If neither is available, returns an error
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
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self::Auto
    }
}

/// Check if the youki binary is available on the system (Linux only)
///
/// Youki is only available on Linux. This function checks:
/// 1. If the youki binary exists in common locations or PATH
///
/// # Returns
/// `true` if youki is available, `false` otherwise
#[cfg(target_os = "linux")]
pub fn is_youki_available() -> bool {
    // Check common locations for youki binary
    let common_paths = [
        "/usr/local/bin/youki",
        "/usr/bin/youki",
        "/opt/youki/bin/youki",
    ];

    for path in common_paths {
        if std::path::Path::new(path).exists() {
            tracing::debug!(path = %path, "Found youki binary");
            return true;
        }
    }

    // Check if youki is in PATH using `which`
    match std::process::Command::new("which").arg("youki").output() {
        Ok(output) if output.status.success() => {
            let path = String::from_utf8_lossy(&output.stdout);
            tracing::debug!(path = %path.trim(), "Found youki in PATH");
            true
        }
        _ => {
            tracing::debug!("youki binary not found");
            false
        }
    }
}

/// Check if the youki binary is available (stub for non-Linux platforms)
#[cfg(not(target_os = "linux"))]
pub fn is_youki_available() -> bool {
    false
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
/// - **Linux**: Tries youki first (if binary is available), falls back to Docker
/// - **Windows/macOS**: Uses Docker directly
/// - If neither runtime is available, returns an error
///
/// # Example
/// ```no_run
/// use zlayer_agent::{RuntimeConfig, YoukiConfig, create_runtime};
///
/// # async fn example() -> Result<(), zlayer_agent::AgentError> {
/// // Auto-select the best available runtime
/// let auto_runtime = create_runtime(RuntimeConfig::Auto).await?;
///
/// // Use mock runtime for testing
/// let mock_runtime = create_runtime(RuntimeConfig::Mock).await?;
///
/// // Use youki runtime explicitly
/// let youki_runtime = create_runtime(
///     RuntimeConfig::Youki(YoukiConfig::default())
/// ).await?;
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
    }
}

/// Automatically select and create the best available runtime
///
/// Selection logic:
/// - On Linux: Try youki first (preferred for performance), fall back to Docker
/// - On Windows/macOS: Use Docker directly (youki is not available)
/// - Returns an error if no runtime is available
async fn create_auto_runtime() -> Result<Arc<dyn Runtime + Send + Sync>> {
    tracing::info!("Auto-selecting container runtime");

    // On Linux, try youki first as it's more performant (no daemon overhead)
    #[cfg(target_os = "linux")]
    {
        if is_youki_available() {
            tracing::info!("Selected youki runtime (Linux-native, no daemon)");
            match YoukiRuntime::new(YoukiConfig::default()).await {
                Ok(runtime) => return Ok(Arc::new(runtime)),
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to initialize youki runtime, trying Docker");
                }
            }
        } else {
            tracing::debug!("youki not available on this Linux system");
        }
    }

    // On non-Linux or if youki failed, try Docker
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
            "No container runtime available. Install youki or start the Docker daemon.".to_string(),
        ))
    }

    #[cfg(all(target_os = "linux", not(feature = "docker")))]
    {
        Err(AgentError::Configuration(
            "No container runtime available. Install youki or enable the 'docker' feature."
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
