// Cross-platform commands
pub mod build;
pub mod lifecycle;
pub mod manager;
pub mod pipeline;
pub mod registry;
pub mod spec;
pub mod tunnel;
pub mod wasm;

// Unix-only commands (depend on zlayer-agent, zlayer-overlay, zlayer-api, etc.)
#[cfg(unix)]
pub mod daemon;
#[cfg(unix)]
pub mod deploy;
#[cfg(unix)]
pub mod exec;
#[cfg(unix)]
pub mod image;
#[cfg(unix)]
pub mod join;
#[cfg(unix)]
pub mod network;
#[cfg(unix)]
pub mod node;
#[cfg(unix)]
pub mod ps;
#[cfg(unix)]
pub mod serve;
#[cfg(unix)]
pub mod system;
#[cfg(unix)]
pub mod token;
#[cfg(unix)]
pub mod volume;
