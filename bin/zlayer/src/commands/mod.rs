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

// Windows-only maintenance commands (WSL2 distro management)
#[cfg(all(target_os = "windows", feature = "wsl"))]
pub mod windows;

pub mod audit_cmd;
pub mod auth;
pub mod container;
pub mod credential;
pub mod daemon;
pub mod deploy;
pub mod env;
pub mod exec;
pub mod group;
pub mod image;
pub mod job;
pub mod join;
pub mod network;
pub mod node;
pub mod notifier;
pub mod permission;
pub mod project;
pub mod ps;
pub mod resolver;
pub mod run;
pub mod secret;
pub mod serve;
pub mod sync_cmd;
pub mod system;
pub mod task;
pub mod token;
pub mod user;
pub mod variable;
pub mod volume;
pub mod workflow;
