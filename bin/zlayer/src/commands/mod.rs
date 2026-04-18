// Cross-platform commands
pub mod build;
pub mod completions;
pub mod lifecycle;
pub mod manager;
pub mod pipeline;
pub mod registry;
pub mod spec;
pub mod tunnel;
pub mod wasm;

// Unix-only commands (depend on zlayer-agent, zlayer-overlay, zlayer-api, etc.)
#[cfg(unix)]
pub mod audit_cmd;
#[cfg(unix)]
pub mod auth;
#[cfg(unix)]
pub mod container;
#[cfg(unix)]
pub mod credential;
#[cfg(unix)]
pub mod daemon;
#[cfg(unix)]
pub mod deploy;
#[cfg(unix)]
pub mod env;
#[cfg(unix)]
pub mod exec;
#[cfg(unix)]
pub mod group;
#[cfg(unix)]
pub mod image;
#[cfg(unix)]
pub mod job;
#[cfg(unix)]
pub mod join;
#[cfg(unix)]
pub mod network;
#[cfg(unix)]
pub mod node;
#[cfg(unix)]
pub mod notifier;
#[cfg(unix)]
pub mod permission;
#[cfg(unix)]
pub mod project;
#[cfg(unix)]
pub mod ps;
#[cfg(unix)]
pub mod resolver;
#[cfg(unix)]
pub mod secret;
#[cfg(unix)]
pub mod serve;
#[cfg(unix)]
pub mod sync_cmd;
#[cfg(unix)]
pub mod system;
#[cfg(unix)]
pub mod task;
#[cfg(unix)]
pub mod token;
#[cfg(unix)]
pub mod user;
#[cfg(unix)]
pub mod variable;
#[cfg(unix)]
pub mod volume;
#[cfg(unix)]
pub mod workflow;
