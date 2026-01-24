//! ZLayer Agent - Container Runtime
//!
//! Manages container lifecycle, health checking, init actions, and proxy integration.

pub mod bundle;
pub mod error;
pub mod health;
pub mod init;
pub mod proxy_manager;
pub mod runtime;
pub mod service;
pub mod youki_runtime;

pub use bundle::*;
pub use error::*;
pub use health::*;
pub use init::*;
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
