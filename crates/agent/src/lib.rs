//! ZLayer Agent - Container Runtime
//!
//! Manages container lifecycle, health checking, init actions, and proxy integration.

pub mod containerd_runtime;
pub mod error;
pub mod health;
pub mod init;
pub mod proxy_manager;
pub mod runtime;
pub mod service;

pub use containerd_runtime::{ContainerdConfig, ContainerdRuntime};
pub use error::*;
pub use health::*;
pub use init::*;
pub use proxy_manager::{ProxyManager, ProxyManagerConfig};
pub use runtime::*;
pub use service::*;

use std::sync::Arc;

/// Configuration for selecting and configuring a container runtime
#[derive(Debug, Clone)]
pub enum RuntimeConfig {
    /// Use the mock runtime for testing and development
    Mock,
    /// Use containerd as the container runtime
    Containerd(ContainerdConfig),
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
/// Returns `AgentError` if the runtime cannot be initialized (e.g., containerd
/// socket not available)
///
/// # Example
/// ```no_run
/// use agent::{RuntimeConfig, ContainerdConfig, create_runtime};
///
/// # async fn example() -> Result<(), agent::AgentError> {
/// // Use mock runtime for testing
/// let mock_runtime = create_runtime(RuntimeConfig::Mock).await?;
///
/// // Use containerd runtime for production
/// let containerd_runtime = create_runtime(
///     RuntimeConfig::Containerd(ContainerdConfig::default())
/// ).await?;
/// # Ok(())
/// # }
/// ```
pub async fn create_runtime(config: RuntimeConfig) -> Result<Arc<dyn Runtime + Send + Sync>> {
    match config {
        RuntimeConfig::Mock => Ok(Arc::new(MockRuntime::new())),
        RuntimeConfig::Containerd(containerd_config) => {
            let runtime = ContainerdRuntime::new(containerd_config).await?;
            Ok(Arc::new(runtime))
        }
    }
}
