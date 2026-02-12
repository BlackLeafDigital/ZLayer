//! Runtime configuration builder.
//!
//! Converts CLI arguments into the runtime configuration used by the agent.

use crate::cli::{Cli, RuntimeType};
#[cfg(target_os = "macos")]
use zlayer_agent::MacSandboxConfig;
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
            state_dir: cli.effective_state_dir(),
            ..Default::default()
        }),
        #[cfg(target_os = "macos")]
        RuntimeType::MacSandbox => RuntimeConfig::MacSandbox(MacSandboxConfig {
            data_dir: cli.effective_data_dir(),
            log_dir: cli.effective_log_dir(),
            gpu_access: true,
        }),
        #[cfg(target_os = "macos")]
        RuntimeType::MacVm => RuntimeConfig::MacVm,
    }
}
