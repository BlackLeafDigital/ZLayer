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
pub mod service;
pub mod youki_runtime;

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
pub use service::*;
pub use youki_runtime::{YoukiConfig, YoukiRuntime};

use std::sync::Arc;

/// Configuration for selecting and configuring a container runtime
#[derive(Debug, Clone)]
pub enum RuntimeConfig {
    /// Use the mock runtime for testing and development
    Mock,
    /// Use youki/libcontainer as the container runtime
    Youki(YoukiConfig),
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self::Mock
    }
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
/// required directories)
///
/// # Example
/// ```no_run
/// use agent::{RuntimeConfig, YoukiConfig, create_runtime};
///
/// # async fn example() -> Result<(), agent::AgentError> {
/// // Use mock runtime for testing
/// let mock_runtime = create_runtime(RuntimeConfig::Mock).await?;
///
/// // Use youki runtime for production
/// let youki_runtime = create_runtime(
///     RuntimeConfig::Youki(YoukiConfig::default())
/// ).await?;
/// # Ok(())
/// # }
/// ```
pub async fn create_runtime(config: RuntimeConfig) -> Result<Arc<dyn Runtime + Send + Sync>> {
    match config {
        RuntimeConfig::Mock => Ok(Arc::new(MockRuntime::new())),
        RuntimeConfig::Youki(youki_config) => {
            let runtime = YoukiRuntime::new(youki_config).await?;
            Ok(Arc::new(runtime))
        }
    }
}
