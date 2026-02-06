//! Runtime configuration builder.
//!
//! Converts CLI arguments into the runtime configuration used by the agent.

use crate::cli::{Cli, RuntimeType};
use zlayer_agent::RuntimeConfig;
#[cfg(target_os = "linux")]
use zlayer_agent::YoukiConfig;

/// Build runtime configuration from CLI arguments
pub(crate) fn build_runtime_config(cli: &Cli) -> RuntimeConfig {
    match cli.runtime {
        RuntimeType::Auto => RuntimeConfig::Auto,
        #[cfg(feature = "docker")]
        RuntimeType::Docker => RuntimeConfig::Docker,
        #[cfg(target_os = "linux")]
        RuntimeType::Youki => RuntimeConfig::Youki(YoukiConfig {
            state_dir: cli.state_dir.clone(),
            ..Default::default()
        }),
    }
}
